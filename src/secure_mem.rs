//! Secret buffers (SECRET-004..006): page-aligned, locked out of the page
//! file, encrypted at rest with `CryptProtectMemory`, zeroed on drop and
//! reachable only through a closure. Also the DPAPI calls behind the token
//! store.
//!
//! This limits exposure; it is not a boundary against code running as the
//! same Windows user, which can read this process's memory or call DPAPI
//! itself (SECRET-012). Non-Windows builds compile a stub that refuses every
//! operation: there is no insecure fallback.

/// The most bytes [`LockedSecret::read_from`] is normally asked to accept:
/// an `op read` result larger than this is refused rather than grown into.
pub const MAX_SECRET_LEN: usize = 64 * 1024;

/// Overwrite `bytes` with zeros in a way the optimiser may not remove.
pub fn wipe(bytes: &mut [u8]) {
    for byte in bytes.iter_mut() {
        // Safety: `byte` is a valid, exclusive reference to one u8.
        unsafe { core::ptr::write_volatile(byte, 0) };
    }
    core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
}

#[cfg(windows)]
pub use imp::{LockedPages, LockedSecret, dpapi_protect, dpapi_unprotect, page_size};
#[cfg(not(windows))]
pub use stub::{LockedSecret, dpapi_protect, dpapi_unprotect};

#[cfg(windows)]
mod imp {
    use std::cell::Cell;
    use std::ffi::c_void;
    use std::fmt;
    use std::io::{self, Read};

    use windows::Win32::Foundation::{ERROR_WORKING_SET_QUOTA, HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CRYPTPROTECTMEMORY_SAME_PROCESS,
        CryptProtectData, CryptProtectMemory, CryptUnprotectData, CryptUnprotectMemory,
    };
    use windows::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
    use windows::Win32::System::Memory::{
        MEM_COMMIT, MEM_RELEASE, MEM_RESERVE, PAGE_READWRITE, VirtualAlloc, VirtualFree,
        VirtualLock, VirtualUnlock,
    };
    use windows::Win32::System::SystemInformation::{GetSystemInfo, SYSTEM_INFO};
    use windows::Win32::System::Threading::{
        GetCurrentProcess, GetProcessWorkingSetSize, SetProcessWorkingSetSize,
    };
    use windows::core::{HRESULT, PCWSTR, s, w};

    use super::wipe;

    /// `WerRegisterExcludedMemoryBlock`.
    type WerRegister = unsafe extern "system" fn(*const c_void, u32) -> HRESULT;
    /// `WerUnregisterExcludedMemoryBlock`.
    type WerUnregister = unsafe extern "system" fn(*const c_void) -> HRESULT;

    /// The system page size.
    #[must_use]
    pub fn page_size() -> usize {
        let mut info = SYSTEM_INFO::default();
        // Safety: `info` is a valid out-parameter.
        unsafe { GetSystemInfo(&raw mut info) };
        usize::try_from(info.dwPageSize).unwrap_or(4096).max(4096)
    }

    /// Look up one export of kernel32 by name at run time. The WER exclusion
    /// calls arrived in Windows 10 2004 and the spec (FR-109) also covers
    /// older Windows 10, where a static import would stop the whole program
    /// from loading.
    fn wer_register() -> Option<WerRegister> {
        // Safety: the module name is a valid constant, and the transmute is
        // between `extern "system"` function pointers with the documented
        // signature of this export.
        unsafe {
            let module = GetModuleHandleW(w!("kernel32.dll")).ok()?;
            let found = GetProcAddress(module, s!("WerRegisterExcludedMemoryBlock"))?;
            Some(core::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                WerRegister,
            >(found))
        }
    }

    /// See [`wer_register`].
    fn wer_unregister() -> Option<WerUnregister> {
        // Safety: as in `wer_register`.
        unsafe {
            let module = GetModuleHandleW(w!("kernel32.dll")).ok()?;
            let found = GetProcAddress(module, s!("WerUnregisterExcludedMemoryBlock"))?;
            Some(core::mem::transmute::<
                unsafe extern "system" fn() -> isize,
                WerUnregister,
            >(found))
        }
    }

    /// Raise this process's working set so `extra` more bytes can be locked.
    fn raise_working_set(extra: usize) -> io::Result<()> {
        let (mut min, mut max) = (0usize, 0usize);
        // Safety: the pseudo-handle is always valid and the out-parameters
        // point at live locals.
        unsafe {
            let process = GetCurrentProcess();
            GetProcessWorkingSetSize(process, &raw mut min, &raw mut max)
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
            SetProcessWorkingSetSize(
                process,
                min.saturating_add(extra),
                max.saturating_add(extra),
            )
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))
        }
    }

    /// Page-aligned, committed, locked memory, zeroed on drop (SECRET-004).
    /// Plaintext-capable: [`LockedSecret`] adds encryption at rest. Holds a
    /// raw pointer, so it is neither `Send` nor `Sync`.
    pub struct LockedPages {
        /// Start of the allocation.
        ptr: *mut u8,
        /// Size of the allocation in bytes, a whole number of pages.
        size: usize,
        /// The block is registered as excluded from crash reports.
        wer_excluded: bool,
    }

    impl fmt::Debug for LockedPages {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("LockedPages(<redacted>)")
        }
    }

    impl LockedPages {
        /// Allocate and lock at least `len` bytes, rounded up to whole pages.
        /// Fails closed: memory that cannot be locked is never handed out.
        ///
        /// # Errors
        /// Allocation failure, or the pages could not be locked even after
        /// raising the working-set quota.
        pub fn new(len: usize) -> io::Result<Self> {
            let page = page_size();
            let size = len
                .max(1)
                .div_ceil(page)
                .checked_mul(page)
                .ok_or_else(|| io::Error::other("secret buffer size overflow"))?;
            // Safety: asks for a fresh private allocation; the result is
            // checked before use.
            let ptr = unsafe { VirtualAlloc(None, size, MEM_COMMIT | MEM_RESERVE, PAGE_READWRITE) }
                .cast::<u8>();
            if ptr.is_null() {
                return Err(io::Error::last_os_error());
            }
            let mut pages = Self {
                ptr,
                size,
                wer_excluded: false,
            };
            // On failure `pages` drops here, which frees the allocation.
            pages.lock()?;
            pages.exclude_from_crash_reports();
            Ok(pages)
        }

        /// `VirtualLock`, raising the working-set quota once if it is the
        /// quota that stopped the lock.
        fn lock(&self) -> io::Result<()> {
            // Safety: the range is this live allocation.
            let first = unsafe { VirtualLock(self.ptr.cast::<c_void>(), self.size) };
            let Err(e) = first else { return Ok(()) };
            if e.code() != ERROR_WORKING_SET_QUOTA.to_hresult() {
                return Err(io::Error::from_raw_os_error(e.code().0));
            }
            // A margin so a burst of small secrets raises the quota rarely.
            raise_working_set(self.size.saturating_add(1024 * 1024))?;
            // Safety: as above.
            unsafe { VirtualLock(self.ptr.cast::<c_void>(), self.size) }
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))
        }

        /// Best effort (SECRET-005): a build without the API, or a full
        /// registration table, leaves the block in crash reports.
        fn exclude_from_crash_reports(&mut self) {
            let Some(register) = wer_register() else {
                return;
            };
            let Ok(size) = u32::try_from(self.size) else {
                return;
            };
            // Safety: the block is this live allocation and is unregistered
            // in `Drop` before it is freed.
            let status = unsafe { register(self.ptr.cast::<c_void>().cast_const(), size) };
            self.wer_excluded = status.is_ok();
        }

        /// Whole allocation as bytes.
        pub fn as_mut_slice(&mut self) -> &mut [u8] {
            // Safety: `ptr..ptr+size` is a live allocation owned by `self`,
            // exclusively borrowed here.
            unsafe { core::slice::from_raw_parts_mut(self.ptr, self.size) }
        }

        /// Whole allocation as UTF-16 units (it is page-aligned, so this is
        /// always aligned for `u16`).
        pub fn as_wide_mut(&mut self) -> &mut [u16] {
            // Safety: as `as_mut_slice`; page alignment satisfies `u16`, and
            // every bit pattern is a valid `u16`.
            unsafe { core::slice::from_raw_parts_mut(self.ptr.cast::<u16>(), self.size / 2) }
        }

        /// Start of the allocation, for handing to `CreateProcessW`.
        #[must_use]
        pub fn as_ptr(&self) -> *const c_void {
            self.ptr.cast::<c_void>().cast_const()
        }

        /// Zero the whole allocation now. `Drop` does this as well; callers
        /// use it to wipe as soon as the contents have been consumed.
        pub fn wipe_now(&mut self) {
            wipe(self.as_mut_slice());
        }

        /// Decrypt the whole allocation in place.
        fn unprotect(&self) -> io::Result<()> {
            let size = u32::try_from(self.size).map_err(io::Error::other)?;
            // Safety: the allocation is live and a whole number of pages, so
            // a multiple of CRYPTPROTECTMEMORY_BLOCK_SIZE.
            unsafe {
                CryptUnprotectMemory(
                    self.ptr.cast::<c_void>(),
                    size,
                    CRYPTPROTECTMEMORY_SAME_PROCESS,
                )
            }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))
        }

        /// Encrypt the whole allocation in place.
        fn protect(&self) -> io::Result<()> {
            let size = u32::try_from(self.size).map_err(io::Error::other)?;
            // Safety: as `unprotect`.
            unsafe {
                CryptProtectMemory(
                    self.ptr.cast::<c_void>(),
                    size,
                    CRYPTPROTECTMEMORY_SAME_PROCESS,
                )
            }
            .map_err(|e| io::Error::from_raw_os_error(e.code().0))
        }

        /// Zero through a shared reference: the reseal guard's last resort.
        fn wipe_shared(&self) {
            for i in 0..self.size {
                // Safety: `i` is inside the live allocation, and nothing else
                // can be reading it while the guard runs.
                unsafe { core::ptr::write_volatile(self.ptr.add(i), 0) };
            }
        }
    }

    impl Drop for LockedPages {
        fn drop(&mut self) {
            self.wipe_now();
            if self.wer_excluded
                && let Some(unregister) = wer_unregister()
            {
                // Safety: registered in `new` with this same address.
                let _ = unsafe { unregister(self.ptr.cast::<c_void>().cast_const()) };
            }
            // Safety: this object owns the allocation and it is not used
            // again. Failure here leaves zeroed memory behind, which is safe.
            unsafe {
                let _ = VirtualUnlock(self.ptr.cast::<c_void>().cast_const(), self.size);
                let _ = VirtualFree(self.ptr.cast::<c_void>(), 0, MEM_RELEASE);
            }
        }
    }

    /// A secret held in [`LockedPages`], encrypted with `CryptProtectMemory`
    /// (same-process) except while a [`LockedSecret::with_plaintext`] closure
    /// runs. It is neither `Clone`, `Send` nor `Sync`, and its `Debug` output
    /// shows neither its contents nor its length (SECRET-006).
    pub struct LockedSecret {
        /// The encrypted pages.
        pages: LockedPages,
        /// Plaintext length within the pages.
        len: usize,
        /// A closure currently holds the plaintext.
        open: Cell<bool>,
        /// Re-encrypting failed, so the value was destroyed.
        destroyed: Cell<bool>,
    }

    impl fmt::Debug for LockedSecret {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("LockedSecret(<redacted>)")
        }
    }

    /// Re-encrypts on every way out of a closure, including a panic.
    struct Reseal<'a>(&'a LockedSecret);

    impl Drop for Reseal<'_> {
        fn drop(&mut self) {
            if self.0.pages.protect().is_err() {
                // Plaintext must not stay behind: destroy the value instead.
                self.0.pages.wipe_shared();
                self.0.destroyed.set(true);
            }
            self.0.open.set(false);
        }
    }

    impl LockedSecret {
        /// Seal `len` bytes already in `pages`.
        fn seal(pages: LockedPages, len: usize) -> io::Result<Self> {
            pages.protect()?;
            Ok(Self {
                pages,
                len,
                open: Cell::new(false),
                destroyed: Cell::new(false),
            })
        }

        /// Copy `bytes` into a new secret. The caller still owns the source
        /// and is responsible for wiping it.
        ///
        /// # Errors
        /// The memory could not be allocated, locked or encrypted.
        pub fn from_slice(bytes: &[u8]) -> io::Result<Self> {
            Self::fill(bytes.len(), |buf| {
                buf[..bytes.len()].copy_from_slice(bytes);
                bytes.len()
            })
        }

        /// Let `write` produce the plaintext directly in locked memory of at
        /// least `capacity` bytes, then encrypt it. `write` returns the
        /// length it wrote.
        ///
        /// # Errors
        /// The memory could not be allocated, locked or encrypted.
        pub fn fill(capacity: usize, write: impl FnOnce(&mut [u8]) -> usize) -> io::Result<Self> {
            let mut pages = LockedPages::new(capacity)?;
            let len = write(pages.as_mut_slice()).min(pages.size);
            Self::seal(pages, len)
        }

        /// Read `reader` to its end directly into locked memory, so no
        /// heap buffer ever holds the value. Grows by moving between locked
        /// allocations; the outgrown one is wiped.
        ///
        /// # Errors
        /// An I/O error, memory failure, or more than `max` bytes
        /// (`ErrorKind::InvalidData`).
        pub fn read_from(reader: &mut impl Read, max: usize) -> io::Result<Self> {
            let mut pages = LockedPages::new(page_size())?;
            let mut len = 0usize;
            loop {
                if len == pages.size {
                    let mut bigger = LockedPages::new(pages.size.saturating_mul(2))?;
                    bigger.as_mut_slice()[..len].copy_from_slice(&pages.as_mut_slice()[..len]);
                    pages = bigger;
                }
                match reader.read(&mut pages.as_mut_slice()[len..]) {
                    Ok(0) => break,
                    Ok(n) => {
                        len += n;
                        if len > max {
                            return Err(io::Error::new(
                                io::ErrorKind::InvalidData,
                                format!("value is larger than the {max} byte limit"),
                            ));
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => return Err(e),
                }
            }
            Self::seal(pages, len)
        }

        /// Plaintext length in bytes.
        #[must_use]
        pub const fn len(&self) -> usize {
            self.len
        }

        /// True when the value has no bytes.
        #[must_use]
        pub const fn is_empty(&self) -> bool {
            self.len == 0
        }

        /// The only way to the value (SECRET-006): decrypt in place, run `f`,
        /// re-encrypt, even if `f` panics. `f` must not keep or copy the
        /// bytes anywhere that is not itself wiped.
        ///
        /// # Errors
        /// A nested call on the same secret, or a failed decryption. `f` does
        /// not run in either case.
        pub fn with_plaintext<R>(&self, f: impl FnOnce(&[u8]) -> R) -> io::Result<R> {
            if self.open.get() {
                return Err(io::Error::other("secret is already open"));
            }
            if self.destroyed.get() {
                return Err(io::Error::other("secret was destroyed"));
            }
            self.pages.unprotect()?;
            self.open.set(true);
            let _reseal = Reseal(self);
            // Safety: `len <= size`, the pages live as long as `self`, and
            // the `open` flag stops any overlapping mutation.
            let bytes = unsafe { core::slice::from_raw_parts(self.pages.ptr, self.len) };
            Ok(f(bytes))
        }

        /// The encrypted pages as they sit in memory.
        #[cfg(test)]
        pub(crate) fn raw_pages(&self) -> &[u8] {
            // Safety: the allocation is live; a test only.
            unsafe { core::slice::from_raw_parts(self.pages.ptr, self.pages.size) }
        }

        /// `VirtualUnlock` fails on pages that are not locked, so this proves
        /// the lock, then restores it.
        #[cfg(test)]
        pub(crate) fn pages_are_locked(&self) -> bool {
            let ptr = self.pages.ptr.cast::<c_void>().cast_const();
            // Safety: the range is this live allocation; a test only.
            unsafe {
                let was_locked = VirtualUnlock(ptr, self.pages.size).is_ok();
                let _ = VirtualLock(ptr, self.pages.size);
                was_locked
            }
        }

        /// The WER registration succeeded.
        #[cfg(test)]
        pub(crate) const fn excluded_from_crash_reports(&self) -> bool {
            self.pages.wer_excluded
        }
    }

    /// A `CRYPT_INTEGER_BLOB` over borrowed bytes.
    fn blob_of(bytes: &[u8]) -> io::Result<CRYPT_INTEGER_BLOB> {
        Ok(CRYPT_INTEGER_BLOB {
            cbData: u32::try_from(bytes.len()).map_err(io::Error::other)?,
            pbData: bytes.as_ptr().cast_mut(),
        })
    }

    /// Encrypt with DPAPI under the current Windows user (SECRET-007).
    /// `entropy` binds the blob to one project, so a token file copied onto
    /// another project's name does not decrypt.
    ///
    /// # Errors
    /// The secret could not be opened, or `CryptProtectData` failed.
    pub fn dpapi_protect(secret: &LockedSecret, entropy: &[u8]) -> io::Result<Vec<u8>> {
        let entropy = blob_of(entropy)?;
        secret.with_plaintext(|plain| {
            let input = blob_of(plain)?;
            let mut output = CRYPT_INTEGER_BLOB::default();
            // Safety: the input blobs borrow live slices for this call, and
            // `output` is a valid out-parameter that this block frees.
            unsafe {
                CryptProtectData(
                    &raw const input,
                    PCWSTR::null(),
                    Some(&raw const entropy),
                    None,
                    None,
                    CRYPTPROTECT_UI_FORBIDDEN,
                    &raw mut output,
                )
                .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
                let len = usize::try_from(output.cbData).map_err(io::Error::other)?;
                let blob = core::slice::from_raw_parts(output.pbData, len).to_vec();
                LocalFree(Some(HLOCAL(output.pbData.cast::<c_void>())));
                Ok(blob)
            }
        })?
    }

    /// Reverse [`dpapi_protect`] straight into a [`LockedSecret`]; the
    /// system's plaintext copy is wiped before it is freed.
    ///
    /// # Errors
    /// The blob is damaged, was made by another user or with other entropy,
    /// or the secret memory failed.
    pub fn dpapi_unprotect(blob: &[u8], entropy: &[u8]) -> io::Result<LockedSecret> {
        let input = blob_of(blob)?;
        let entropy = blob_of(entropy)?;
        let mut output = CRYPT_INTEGER_BLOB::default();
        // Safety: the input blobs borrow live slices for this call; `output`
        // is a valid out-parameter.
        unsafe {
            CryptUnprotectData(
                &raw const input,
                None,
                Some(&raw const entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &raw mut output,
            )
        }
        .map_err(|e| io::Error::from_raw_os_error(e.code().0))?;
        let len = usize::try_from(output.cbData).map_err(io::Error::other)?;
        // Safety: on success `output` is a LocalAlloc block of `len` bytes
        // that this function owns and frees.
        unsafe {
            let plain = core::slice::from_raw_parts_mut(output.pbData, len);
            let secret = LockedSecret::from_slice(plain);
            wipe(plain);
            LocalFree(Some(HLOCAL(output.pbData.cast::<c_void>())));
            secret
        }
    }
}

#[cfg(not(windows))]
mod stub {
    use std::fmt;
    use std::io::{self, Read};

    /// Why every stub operation fails.
    fn unsupported<T>() -> io::Result<T> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "secure secret memory is only implemented on Windows",
        ))
    }

    /// Stub: can never be constructed, so no secret can exist off Windows.
    pub struct LockedSecret(());

    impl fmt::Debug for LockedSecret {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("LockedSecret(<redacted>)")
        }
    }

    impl LockedSecret {
        /// Always fails off Windows.
        ///
        /// # Errors
        /// Always `ErrorKind::Unsupported`.
        pub fn from_slice(_bytes: &[u8]) -> io::Result<Self> {
            unsupported()
        }

        /// Always fails off Windows.
        ///
        /// # Errors
        /// Always `ErrorKind::Unsupported`.
        pub fn fill(_capacity: usize, _write: impl FnOnce(&mut [u8]) -> usize) -> io::Result<Self> {
            unsupported()
        }

        /// Always fails off Windows.
        ///
        /// # Errors
        /// Always `ErrorKind::Unsupported`.
        pub fn read_from(_reader: &mut impl Read, _max: usize) -> io::Result<Self> {
            unsupported()
        }

        /// Always 0.
        #[must_use]
        pub const fn len(&self) -> usize {
            0
        }

        /// Always true.
        #[must_use]
        pub const fn is_empty(&self) -> bool {
            true
        }

        /// Always fails off Windows.
        ///
        /// # Errors
        /// Always `ErrorKind::Unsupported`.
        pub fn with_plaintext<R>(&self, _f: impl FnOnce(&[u8]) -> R) -> io::Result<R> {
            unsupported()
        }
    }

    /// Always fails off Windows.
    ///
    /// # Errors
    /// Always `ErrorKind::Unsupported`.
    pub fn dpapi_protect(_secret: &LockedSecret, _entropy: &[u8]) -> io::Result<Vec<u8>> {
        unsupported()
    }

    /// Always fails off Windows.
    ///
    /// # Errors
    /// Always `ErrorKind::Unsupported`.
    pub fn dpapi_unprotect(_blob: &[u8], _entropy: &[u8]) -> io::Result<LockedSecret> {
        unsupported()
    }
}

#[cfg(all(test, windows))]
#[path = "secure_mem_tests.rs"]
mod tests;
