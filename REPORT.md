From the latest boot log, the key fact is:

qsh (pid 3) is not exiting anymore
It reaches:
Qunix OS 0.2.0 - A Rust OS
Type 'help' for commands.
user@qunix:/#

So the previous “exit_group on startup” issue is already gone.

What is actually happening now

You now have a TTY/input loop problem, not a process lifecycle problem.

The critical pattern:

[pid=3 nr=0(read)]
[pid=3 nr=0(read)]
[pid=3 nr=0(read)]
...

This means:

qsh is alive
it is repeatedly calling read(0, ...)
but it is receiving no blocking input behavior or immediate EOF-like return
Root cause classification (based on behavior)

This is no longer:

❌ execve / iret bug
❌ syscall table corruption (mostly fixed)
❌ scheduler exit bug

It is one of these 3:

1. fd 0 is NOT a real blocking TTY

Most likely.

Even if /dev/tty exists, fd 0 may actually be:

a pipe
a file-backed tmpfs node
or a tty device without blocking semantics wired
2. TTY read returns “0 bytes ready” incorrectly

Your symptom strongly matches:

kernel returns 0 instead of blocking
shell interprets 0 as EOF → loops / exits logic / re-read spam

In POSIX:

0 from read() = EOF
blocking TTY should NOT return 0 unless hangup

So this is a semantic violation in your TTY layer.

3. session/controlling terminal not actually attached

Even if:

setsid() exists
TIOCSCTTY exists

your earlier logs already showed:

setsid() syscall not actually wired earlier

So likely now:

qsh THINKS it owns tty
kernel does NOT mark controlling terminal
so terminal driver behaves like “detached device”

Result:

read() returns empty / immediate 0 / non-blocking fallback path
Strongest evidence from your log

This line is decisive:

user@qunix:/#

Then immediately:

pid=3 nr=0(read)

That means:

shell printed prompt
then immediately tried to read input
but no blocking wait happened

So the bug is:

“TTY is not blocking on input availability”

Where the bug is almost certainly located

You should focus on these 3 places (in order):

1. TTY driver read implementation

Check for:

missing wait queue / sleep path
missing blocking condition
returning 0 instead of blocking

You want logic like:

if no input → sleep task
NOT return 0
2. file descriptor flags for fd 0

Check:

O_NONBLOCK accidentally set?
wrong file->ops assigned?
fd 0 pointing to wrong inode type?
3. execve fd inheritance path

Even if init does correct:

dup2(tty → 0/1/2)

verify kernel actually preserves:

file type = TTY device
not cloned as regular file object
Minimal diagnosis conclusion

Current system state:

kernel is stable ✔
execve works ✔
fork works ✔
syscalls work ✔
shell starts ✔
prompt prints ✔

But:

stdin is not behaving like a blocking terminal stream

So read(0) is effectively:

return 0 immediately OR spin without blocking

instead of:

block until input arrives
What to do next (precise target fix)

Instrument ONLY this:

sys_read path (pid 3, fd 0)

Log:

fd type (tty/file/pipe)
blocking flag
return value reason:
EAGAIN?
EOF?
immediate return?

Then fix based on outcome:

If fd is TTY but returns 0:

→ fix TTY blocking scheduler sleep path

If fd is not TTY:

→ fix dup2/execve fd inheritance

If O_NONBLOCK:

→ fix file flags in init launch

but issues is keyborad is not typing in shell