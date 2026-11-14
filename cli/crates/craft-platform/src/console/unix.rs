use super::InputEvent;
use nix::fcntl::{FcntlArg, OFlag, fcntl};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::termios::{
    FlushArg, InputFlags, LocalFlags, SetArg, SpecialCharacterIndices, Termios, tcflush, tcgetattr,
    tcsetattr,
};
use std::fs::File;
use std::io;
use std::os::fd::AsFd;
use std::time::Duration;
use zeroize::{Zeroize, Zeroizing};

pub(super) struct NativeConsole {
    input: File,
    original: Termios,
    flags: OFlag,
    pending: Zeroizing<[u8; 4]>,
    length: usize,
}

fn error(value: nix::errno::Errno) -> io::Error {
    io::Error::from_raw_os_error(value as i32)
}

impl NativeConsole {
    pub(super) fn open() -> io::Result<Self> {
        let input = File::from(io::stdin().as_fd().try_clone_to_owned()?);
        let original = tcgetattr(&input).map_err(error)?;
        let flags = OFlag::from_bits_retain(fcntl(&input, FcntlArg::F_GETFL).map_err(error)?);
        let mut mode = original.clone();
        mode.local_flags
            .remove(LocalFlags::ECHO | LocalFlags::ECHONL | LocalFlags::ICANON | LocalFlags::ISIG);
        mode.input_flags.remove(
            InputFlags::IXON
                | InputFlags::IXOFF
                | InputFlags::ICRNL
                | InputFlags::INLCR
                | InputFlags::IGNCR
                | InputFlags::ISTRIP,
        );
        mode.control_chars[SpecialCharacterIndices::VMIN as usize] = 0;
        mode.control_chars[SpecialCharacterIndices::VTIME as usize] = 0;
        fcntl(&input, FcntlArg::F_SETFL(flags | OFlag::O_NONBLOCK)).map_err(error)?;
        if let Err(failure) = tcsetattr(&input, SetArg::TCSANOW, &mode) {
            fcntl(&input, FcntlArg::F_SETFL(flags)).map_err(error)?;
            return Err(error(failure));
        }
        Ok(Self {
            input,
            original,
            flags,
            pending: Zeroizing::new([0; 4]),
            length: 0,
        })
    }

    pub(super) fn poll(&mut self, wait: Duration) -> io::Result<Option<InputEvent>> {
        let mut descriptors = [PollFd::new(self.input.as_fd(), PollFlags::POLLIN)];
        let count = poll(
            &mut descriptors,
            PollTimeout::from(wait.as_millis().min(100) as u16),
        )
        .map_err(error)?;
        if count == 0 {
            return Ok(None);
        }
        let ready = descriptors[0].revents().unwrap_or(PollFlags::empty());
        if ready.intersects(PollFlags::POLLERR | PollFlags::POLLHUP | PollFlags::POLLNVAL) {
            return Err(io::Error::other("Terminal input ended"));
        }
        if self.length >= self.pending.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "Invalid terminal encoding",
            ));
        }
        match nix::unistd::read(&self.input, &mut self.pending[self.length..self.length + 1]) {
            Ok(0) => {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Terminal input ended",
                ));
            }
            Ok(_) => self.length += 1,
            Err(nix::errno::Errno::EAGAIN) | Err(nix::errno::Errno::EINTR) => return Ok(None),
            Err(failure) => return Err(error(failure)),
        }
        let text = match std::str::from_utf8(&self.pending[..self.length]) {
            Ok(text) => text,
            Err(failure) if failure.error_len().is_none() => return Ok(None),
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "Invalid terminal encoding",
                ));
            }
        };
        let character = text
            .chars()
            .next()
            .ok_or_else(|| io::Error::other("Empty terminal event"))?;
        let mut units = Zeroizing::new([0; 2]);
        let length = character.encode_utf16(&mut *units).len();
        self.pending.zeroize();
        self.length = 0;
        if length == 1 {
            Ok(super::event(units[0], 1))
        } else {
            Ok(Some(InputEvent::Text {
                units,
                length,
                repeat: 1,
            }))
        }
    }

    pub(super) fn restore(&mut self) -> io::Result<()> {
        let flushed = tcflush(&self.input, FlushArg::TCIFLUSH).map_err(error);
        let mode = tcsetattr(&self.input, SetArg::TCSANOW, &self.original).map_err(error);
        let flags = fcntl(&self.input, FcntlArg::F_SETFL(self.flags)).map_err(error);
        mode?;
        flags?;
        flushed?;
        Ok(())
    }
}
