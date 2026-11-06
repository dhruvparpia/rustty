//! A window started without activation still gets its renderer and a real size.
//! Nothing held COM open on the UI thread, and only activating a window happened
//! to start it, so a "Run minimized" shortcut opened a window that could not draw.
//! That window also remembered Windows' parking spot for minimized windows as its
//! normal size, and came back 160 pixels wide.
#![cfg(windows)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

use std::process::Command;

/// Starts rustty minimized with scratch settings, asks it to save as if Windows
/// were shutting down, and returns what it printed, its restored width, and its
/// saved session. Only the process started here is stopped.
fn run_minimized(name: &str) -> (String, i32, String) {
    let scratch = std::env::temp_dir().join(format!("rustty-min-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&scratch);
    std::fs::create_dir_all(scratch.join("rustty")).expect("scratch settings");
    let stderr = scratch.join("stderr.txt");
    let width = scratch.join("width.txt");
    let script = format!(
        r#"
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class Min {{
 [StructLayout(LayoutKind.Sequential)] public struct P {{ public int Len, Flags, Show, MinX, MinY, MaxX, MaxY, NL, NT, NR, NB; }}
 [DllImport("user32.dll")] public static extern bool GetWindowPlacement(IntPtr h, ref P p);
 [DllImport("user32.dll")] public static extern IntPtr SendMessageW(IntPtr h, uint m, IntPtr w, IntPtr l);
}}
'@
$env:APPDATA='{dir}'; $env:LOCALAPPDATA='{dir}'
$p = Start-Process -FilePath '{exe}' -WindowStyle Minimized -PassThru -RedirectStandardError '{err}'
$deadline = (Get-Date).AddSeconds(10)
while ($p.MainWindowHandle -eq 0 -and (Get-Date) -lt $deadline) {{ Start-Sleep -Milliseconds 100; $p.Refresh() }}
Start-Sleep -Seconds 3
$pl = New-Object Min+P; $pl.Len = 44
[void][Min]::GetWindowPlacement($p.MainWindowHandle, [ref]$pl)
Set-Content -Path '{width}' -Value ($pl.NR - $pl.NL)
[void][Min]::SendMessageW($p.MainWindowHandle, 0x11, [IntPtr]::Zero, [IntPtr]::Zero)
$kids = Get-CimInstance Win32_Process -Filter "ParentProcessId=$($p.Id)"
Stop-Process -Id $p.Id -Force
foreach ($k in $kids) {{ Stop-Process -Id $k.ProcessId -Force -ErrorAction SilentlyContinue }}
"#,
        dir = scratch.display(),
        exe = env!("CARGO_BIN_EXE_rustty"),
        err = stderr.display(),
        width = width.display(),
    );
    let status = Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .status()
        .expect("powershell runs");
    assert!(status.success(), "the launch script failed: {status}");
    let said = std::fs::read_to_string(&stderr).expect("stderr was captured");
    let restored = std::fs::read_to_string(&width)
        .expect("width was captured")
        .trim()
        .parse()
        .expect("a width");
    let session =
        std::fs::read_to_string(scratch.join("rustty").join("session.toml")).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&scratch);
    (said, restored, session)
}

#[test]
#[ignore = "needs a desktop session: cargo test --test minimized_start -- --ignored"]
fn a_minimized_start_gets_a_renderer_and_a_real_size() {
    let (said, restored, session) = run_minimized("start");
    assert!(!said.contains("renderer unavailable"), "{said}");
    assert!(restored >= 400, "it would come back {restored} wide");
    // Break caught: a save while minimized writing the parking spot.
    assert!(session.contains("[window]"), "{session}");
    assert!(!session.contains("-32000"), "{session}");
}
