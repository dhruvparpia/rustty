//! Tests for `secure_mem` (SECRET-004..006). Windows only: they exercise
//! the real `VirtualLock`, `CryptProtectMemory` and DPAPI calls, and hold
//! only throwaway values.

use std::io;

use super::*;

/// A test value long enough that a chance match in ciphertext is impossible.
const PLAIN: &[u8] = b"not-a-real-secret-0123456789-abcdefghijklmnop";

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn plaintext_round_trips_through_the_closure() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    assert_eq!(secret.len(), PLAIN.len());
    assert!(!secret.is_empty());
    let seen = secret.with_plaintext(<[u8]>::to_vec).unwrap();
    assert_eq!(seen, PLAIN);
}

#[test]
fn value_is_encrypted_at_rest_and_again_after_each_use() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    assert!(
        !contains(secret.raw_pages(), PLAIN),
        "plaintext must not sit in the locked pages between uses"
    );
    secret
        .with_plaintext(|bytes| assert_eq!(bytes, PLAIN))
        .unwrap();
    assert!(
        !contains(secret.raw_pages(), PLAIN),
        "the buffer must be re-encrypted when the closure returns"
    );
    // Re-encrypting did not corrupt it.
    assert_eq!(secret.with_plaintext(<[u8]>::to_vec).unwrap(), PLAIN);
}

#[test]
fn a_panicking_closure_still_re_encrypts() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        secret
            .with_plaintext(|_| panic!("closure failure"))
            .unwrap();
    }));
    assert!(outcome.is_err());
    assert!(!contains(secret.raw_pages(), PLAIN));
    assert_eq!(secret.with_plaintext(<[u8]>::to_vec).unwrap(), PLAIN);
}

#[test]
fn nested_access_to_the_same_secret_is_refused() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    let inner = secret
        .with_plaintext(|_| secret.with_plaintext(|_| ()).is_err())
        .unwrap();
    assert!(inner, "a nested decrypt would corrupt the buffer");
    assert_eq!(secret.with_plaintext(<[u8]>::to_vec).unwrap(), PLAIN);
}

#[test]
fn pages_are_page_aligned_and_locked() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    let pages = secret.raw_pages();
    let page = page_size();
    assert_eq!(pages.as_ptr() as usize % page, 0);
    assert_eq!(pages.len() % page, 0);
    assert!(secret.pages_are_locked());
}

#[test]
fn pages_are_excluded_from_crash_reports_on_this_windows_build() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    assert!(secret.excluded_from_crash_reports());
}

#[test]
fn wipe_zeroes_every_byte() {
    let mut bytes = PLAIN.to_vec();
    wipe(&mut bytes);
    assert!(bytes.iter().all(|b| *b == 0));
}

#[test]
fn locked_pages_are_zeroed_by_their_wipe_step() {
    let mut pages = LockedPages::new(32).unwrap();
    pages.as_mut_slice()[..32].copy_from_slice(&PLAIN[..32]);
    pages.wipe_now();
    assert!(pages.as_mut_slice().iter().all(|b| *b == 0));
}

#[test]
fn debug_output_never_shows_contents_or_length() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    let debug = format!("{secret:?}");
    assert!(!debug.contains("not-a-real-secret"));
    assert!(!debug.contains(&PLAIN.len().to_string()));
    assert!(debug.contains("redacted"));
}

#[test]
fn read_from_fills_a_secret_directly_and_grows_past_one_page() {
    let big = vec![b'x'; page_size() * 2 + 17];
    let mut reader = big.as_slice();
    let secret = LockedSecret::read_from(&mut reader, MAX_SECRET_LEN).unwrap();
    assert_eq!(secret.len(), big.len());
    assert_eq!(secret.with_plaintext(<[u8]>::to_vec).unwrap(), big);
    assert!(secret.pages_are_locked());
}

#[test]
fn read_from_an_empty_source_gives_an_empty_secret() {
    let mut reader: &[u8] = b"";
    let secret = LockedSecret::read_from(&mut reader, MAX_SECRET_LEN).unwrap();
    assert!(secret.is_empty());
}

#[test]
fn read_from_refuses_more_than_the_limit() {
    let big = vec![b'x'; 5000];
    let mut reader = big.as_slice();
    let err = LockedSecret::read_from(&mut reader, 4096).err();
    assert_eq!(err.map(|e| e.kind()), Some(io::ErrorKind::InvalidData));
}

#[test]
fn fill_writes_into_locked_memory_then_encrypts() {
    let secret = LockedSecret::fill(PLAIN.len(), |buf| {
        buf[..PLAIN.len()].copy_from_slice(PLAIN);
        PLAIN.len()
    })
    .unwrap();
    assert!(!contains(secret.raw_pages(), PLAIN));
    assert_eq!(secret.with_plaintext(<[u8]>::to_vec).unwrap(), PLAIN);
}

#[test]
fn many_secrets_can_be_locked_at_once() {
    // More pages than the default minimum working set allows: the quota is
    // raised rather than the lock being skipped.
    let held: Vec<LockedSecret> = (0..100)
        .map(|_| LockedSecret::from_slice(PLAIN).unwrap())
        .collect();
    assert!(held.iter().all(LockedSecret::pages_are_locked));
}

#[test]
fn dpapi_round_trips_and_the_blob_hides_the_value() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    let blob = dpapi_protect(&secret, b"vault-a").unwrap();
    assert!(!contains(&blob, PLAIN));
    let back = dpapi_unprotect(&blob, b"vault-a").unwrap();
    assert_eq!(back.with_plaintext(<[u8]>::to_vec).unwrap(), PLAIN);
}

#[test]
fn dpapi_blob_is_bound_to_its_entropy() {
    let secret = LockedSecret::from_slice(PLAIN).unwrap();
    let blob = dpapi_protect(&secret, b"vault-a").unwrap();
    assert!(dpapi_unprotect(&blob, b"vault-b").is_err());
}
