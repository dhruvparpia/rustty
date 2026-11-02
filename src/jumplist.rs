//! Windows Jump List (roadmap v0.2 "Windows Jump List (native taskbar
//! API)"): the taskbar right-click menu gets "New window" plus one task
//! per configured shell profile. Tasks launch this exe with
//! `--profile <name>`, so the app must parse that flag (see `cli`).
//!
//! The list is built in memory through `ICustomDestinationList`; no
//! `.lnk` files are written by hand. Any failure is reported by the
//! caller and never blocks startup.

use std::collections::BTreeSet;

use windows::Win32::Foundation::RPC_E_CHANGED_MODE;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
    CoUninitialize,
};
use windows::Win32::UI::Shell::Common::{IObjectArray, IObjectCollection};
use windows::Win32::UI::Shell::{
    DestinationList, EnumerableObjectCollection, ICustomDestinationList, IShellLinkW, ShellLink,
};
use windows::core::{HSTRING, Interface};

/// One Jump List task: a label and the command line for this exe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JumpTask {
    /// Label shown in the Jump List.
    pub title: String,
    /// Arguments passed to the executable (may be empty).
    pub arguments: String,
}

/// Build the task list: a plain "New window" task, one `--profile <name>`
/// task per profile, then one `--workspace <name>` task per workspace
/// template (WORKSPACE-005). Duplicate names (case-insensitive) collapse
/// to one task, matching how profiles merge by name.
#[must_use]
pub fn tasks_for(profiles: &[String], workspaces: &[String]) -> Vec<JumpTask> {
    let mut tasks = vec![JumpTask {
        title: String::from("New window"),
        arguments: String::new(),
    }];
    let mut seen = BTreeSet::new();
    for name in profiles {
        if seen.insert(name.to_lowercase()) {
            tasks.push(JumpTask {
                title: format!("New {name} window"),
                arguments: format!("--profile \"{name}\""),
            });
        }
    }
    for name in workspaces {
        if seen.insert(name.to_lowercase()) {
            tasks.push(JumpTask {
                title: format!("Open {name}"),
                arguments: format!("--workspace \"{name}\""),
            });
        }
    }
    tasks
}

/// Install the Jump List for `app_id` (the app's AUMID) running from
/// `exe`. Errors from any COM step are returned; the caller logs them.
pub fn build(app_id: &str, exe: &str, tasks: &[JumpTask]) -> windows::core::Result<()> {
    // COM is per-thread: initialise for this call, undo it afterwards.
    // RPC_E_CHANGED_MODE means COM is already running on this thread
    // under a different model; the list builds fine, and that apartment
    // is not ours to uninitialise.
    let co = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if co.is_err() && co != RPC_E_CHANGED_MODE {
        return Err(co.into());
    }
    let result = build_inner(app_id, exe, tasks);
    if co.is_ok() {
        unsafe { CoUninitialize() };
    }
    result
}

fn build_inner(app_id: &str, exe: &str, tasks: &[JumpTask]) -> windows::core::Result<()> {
    let app_id = HSTRING::from(app_id);
    let exe = HSTRING::from(exe);
    let dest: ICustomDestinationList =
        unsafe { CoCreateInstance(&DestinationList, None, CLSCTX_INPROC_SERVER) }?;
    // SetAppID must precede BeginList (documented contract).
    unsafe { dest.SetAppID(&app_id) }?;
    // BeginList hands back the destinations the user just cleared from
    // the list. shortcut: they are not deleted from disk; harmless for
    // tasks that were never persisted. Upgrade if custom destinations
    // (file entries) are ever added.
    let _removed: IObjectArray = unsafe { dest.BeginList(&mut 0u32) }?;
    for task in tasks {
        let link: IShellLinkW =
            unsafe { CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER) }?;
        unsafe {
            link.SetPath(&exe)?;
            if !task.arguments.is_empty() {
                link.SetArguments(&HSTRING::from(&task.arguments))?;
            }
            link.SetDescription(&HSTRING::from(&task.title))?;
        }
        // Each task goes in its own one-object collection: AddUserTasks
        // runs the task as a launch, and Windows caps the visible count.
        let collection: IObjectCollection =
            unsafe { CoCreateInstance(&EnumerableObjectCollection, None, CLSCTX_INPROC_SERVER) }?;
        unsafe { collection.AddObject(&link) }?;
        let as_array: IObjectArray = collection.cast()?;
        unsafe { dest.AddUserTasks(&as_array) }?;
    }
    unsafe { dest.CommitList() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tasks_start_with_a_plain_new_window() {
        let tasks = tasks_for(&[String::from("pwsh"), String::from("Git Bash")], &[]);
        assert_eq!(tasks[0].title, "New window");
        assert!(tasks[0].arguments.is_empty());
        assert_eq!(tasks[1].title, "New pwsh window");
        assert_eq!(tasks[1].arguments, "--profile \"pwsh\"");
        assert_eq!(tasks[2].title, "New Git Bash window");
    }

    #[test]
    fn duplicate_profile_names_collapse() {
        let tasks = tasks_for(&[String::from("pwsh"), String::from("PWSH")], &[]);
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn workspace_tasks_launch_with_the_workspace_flag() {
        let tasks = tasks_for(&["pwsh".into()], &["Web App".into(), "web app".into()]);
        assert_eq!(tasks[2].title, "Open Web App");
        assert_eq!(tasks[2].arguments, "--workspace \"Web App\"");
        assert_eq!(tasks.len(), 3, "case-insensitive duplicate collapses");
    }
}
