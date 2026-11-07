//! A GUI launch releases the console the subsystem allocated. Printing after
//! that must never panic: a double-click would otherwise crash the app on the
//! first diagnostic it prints. Runs in a child process with a console of its
//! own, because the release affects the whole process and inherited pipes
//! would hide the failure.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::process::Command;
use std::time::{Duration, Instant};

/// Set in the child: the file to create once it has printed without panicking.
const MARKER_VAR: &str = "RUSTTY_TEST_CONSOLE_MARKER";
const TEST_NAME: &str = "printing_after_releasing_the_console_does_not_panic";

#[test]
fn printing_after_releasing_the_console_does_not_panic() {
    if let Some(marker) = std::env::var_os(MARKER_VAR) {
        rustty::cli::free_console();
        // The app opens plenty after this, and Windows reuses handle numbers.
        // A read-only file now sitting on a stale stderr handle is what turns
        // the write into "Access is denied", and a panic.
        let exe = std::env::current_exe().expect("test exe");
        let _files: Vec<_> = (0..200)
            .map(|_| std::fs::File::open(&exe).expect("open"))
            .collect();
        eprintln!("rustty: a diagnostic after the console is gone");
        println!("and some output");
        std::fs::write(marker, b"ok").expect("marker");
        return;
    }
    let marker = std::env::temp_dir().join(format!("rustty-console-{}", std::process::id()));
    let _ = std::fs::remove_file(&marker);
    let exe = std::env::current_exe().expect("test exe");
    // A shell launch, as a double-click does: the child gets its own console
    // and none of this process's pipes.
    Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "Start-Process -WindowStyle Hidden -FilePath '{}' -ArgumentList '--exact','{TEST_NAME}','--nocapture'",
                exe.display()
            ),
        ])
        .env(MARKER_VAR, &marker)
        .status()
        .expect("start child");
    let deadline = Instant::now() + Duration::from_secs(20);
    while !marker.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let printed = marker.exists();
    let _ = std::fs::remove_file(&marker);
    assert!(printed, "the child panicked while printing");
}
