use std::{
    io::Read,
    process::{Command, ExitStatus, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

#[cfg(unix)]
use std::{io, process::Child};

pub fn read_bounded(
    command: &mut Command,
    operation: &str,
    timeout: Duration,
    limit: u64,
) -> crate::Result<(ExitStatus, Vec<u8>)> {
    command.stdout(Stdio::piped());
    ProcessTree::configure(command);
    let mut child = command.spawn()?;
    let tree = match ProcessTree::attach(&child) {
        Ok(tree) => tree,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("could not contain {operation}: {error}").into());
        }
    };
    let stdout = child.stdout.take().expect("stdout is piped");
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = stdout.take(limit + 1).read_to_end(&mut bytes);
        let _ = sender.send(result.map(|_| bytes));
    });
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let mut bytes = None;
        let mut status = None;
        let mut tree = Some(tree);
        loop {
            if status.is_none() {
                status = child.try_wait()?;
                if status.is_some() {
                    drop(tree.take());
                }
            }
            if bytes.is_none() {
                match receiver.try_recv() {
                    Ok(output) => {
                        let output = output?;
                        if output.len() as u64 > limit {
                            return crate::reject(format!(
                                "{operation} output exceeded its byte limit"
                            ));
                        }
                        bytes = Some(output);
                    }
                    Err(mpsc::TryRecvError::Empty) => (),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        return crate::reject(format!(
                            "{operation} output reader stopped unexpectedly"
                        ));
                    }
                }
            }
            if let (Some(status), Some(bytes)) = (status, &mut bytes) {
                return Ok((status, std::mem::take(bytes)));
            }
            if Instant::now() >= deadline {
                return crate::reject(format!("{operation} timed out"));
            }
            thread::sleep(Duration::from_millis(20));
        }
    })();
    let _ = child.wait();
    result
}

#[cfg(unix)]
pub struct ProcessTree(nix::unistd::Pid);

#[cfg(unix)]
impl ProcessTree {
    pub fn configure(command: &mut Command) {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }

    pub fn attach(child: &Child) -> io::Result<Self> {
        let pid = i32::try_from(child.id())
            .map_err(|_| io::Error::other("unsupported child process ID"))?;
        Ok(Self(nix::unistd::Pid::from_raw(pid)))
    }
}

#[cfg(unix)]
impl Drop for ProcessTree {
    fn drop(&mut self) {
        let _ = nix::sys::signal::killpg(self.0, nix::sys::signal::Signal::SIGKILL);
    }
}

#[cfg(unix)]
pub fn stop_orphans() {
    let group = nix::unistd::getpgrp();
    if group == nix::unistd::getpid() {
        let _ = nix::sys::signal::killpg(group, nix::sys::signal::Signal::SIGKILL);
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod windows {
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::process::{Child, Command};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectExtendedLimitInformation,
        SetInformationJobObject,
    };

    pub struct ProcessTree {
        _job: OwnedHandle,
    }

    impl ProcessTree {
        pub fn configure(_: &mut Command) {}

        pub fn attach(child: &Child) -> io::Result<Self> {
            // An unnamed job has no caller-provided attributes or name to dereference.
            let raw = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if raw.is_null() {
                return Err(io::Error::last_os_error());
            }
            // CreateJobObjectW returns a new handle. OwnedHandle closes it on every exit path.
            let job = unsafe { OwnedHandle::from_raw_handle(raw) };
            let mut limits = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            // The pointed-to structure remains live for the synchronous call and its size matches.
            if unsafe {
                SetInformationJobObject(
                    job.as_raw_handle(),
                    JobObjectExtendedLimitInformation,
                    (&limits as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                    std::mem::size_of_val(&limits) as u32,
                )
            } == 0
            {
                return Err(io::Error::last_os_error());
            }
            // Both handles remain owned through the call. The child awaits the launch permit.
            if unsafe { AssignProcessToJobObject(job.as_raw_handle(), child.as_raw_handle()) } == 0
            {
                return Err(io::Error::last_os_error());
            }
            Ok(Self { _job: job })
        }
    }
}

#[cfg(windows)]
pub use windows::ProcessTree;

#[cfg(windows)]
pub fn stop_orphans() {
    // The Job Object closes and terminates its processes when the launcher exits.
}
