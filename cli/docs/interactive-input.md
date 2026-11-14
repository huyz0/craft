# Internal interactive credential input

The runtime now has an explicit `read_service_credentials_interactively` entry point. It is internal plumbing for service provisioning. Public provision/login/refresh/logout commands are still unfinished; ordinary agent commands never invoke this prompt.

The entry point requires terminal stdin, then displays fixed labels on stderr for the client ID and secret. Both fields are hidden. It writes no credential, field length or placeholder characters to stdout/stderr and returns the same opaque credential type as the secure pipe reader.

## Input and restoration

Enter completes a field. Backspace removes a complete UTF-16 surrogate pair when present. Ctrl-U clears the field. Ctrl-C, Ctrl-D, Ctrl-Z and Escape cancel entry. Other control characters remain invalid credential data rather than being silently removed. Spaces and Unicode text are preserved without trimming or normalization.

Client ID and secret limits remain 1 KiB and 64 KiB of UTF-8. The reader preallocates zeroizing UTF-16 and UTF-8 buffers and checks bounds before extending them. A malformed surrogate sequence, excessive input, read/write failure, cancellation or deadline refuses credentials. Errors never include native I/O text or entered values.

One process-local guard owns terminal input on the calling thread. It saves native state, disables echo/line processing and polls bounded input events while the runtime checks the invocation budget. Pending prompt input is discarded before echo returns, including on cancellation. Checked restoration precedes returning credentials or an input error. Restoration failure refuses credential use and gives terminal-reset guidance; guard drop provides a best-effort fallback.

The Windows adapter uses verified console mode operations, a bounded aligned zeroizing `INPUT_RECORD` buffer and wide key events. The Unix adapter uses saved termios/file flags, nonblocking UTF-8 input and a bounded decoder. Only the Windows adapter has been compiled and natively exercised here.

The caller must own foreground input exclusively, including against other processes/readers. The process-local mutex cannot fence another application reading the same terminal. Forced process termination can bypass drop/restoration. The guard does not claim protection against same-principal memory inspection or every transient compiler/parser allocation.

## Reproduce Windows native acceptance

Run from `cli/` in an authorized native Windows development environment:

```powershell
& .\tools\console-acceptance.ps1
```

The script builds the current platform test executable, verifies its workspace location and launches it in a hidden console. The child refuses to proceed unless it is the console's only attached client. It uses synthetic events to verify Unicode, disabled echo, discarded pending input, bounded empty polling and exact mode restoration on finish/drop. It inspects its own console buffer without printing or persisting screen text. The script bounds the owned child to fifteen seconds and restores its fixture environment variable.

Ordinary verification ignores this test because it requires its own console. Runtime fixtures separately exercise field collection, Unicode editing, byte limits, cancellation/deadline, I/O failures and refusal after restoration failure. [Observed evidence](evidence/interactive-input.md) records these scopes separately from live OAuth acceptance and Unix qualification.
