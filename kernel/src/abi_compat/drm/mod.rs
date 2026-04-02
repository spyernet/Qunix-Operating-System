//! Display ioctl bridge: routes DRM-class ioctls from non-DRM fds to the
//! Qunix display subsystem. Real DRM dispatch → crate::drm::drm_ioctl.

/// Called for ioctls on non-DRM fds that happen to have DRM ioctl numbers.
/// Also handles generic TTY ioctls needed by X11/Wayland on /dev/tty.
pub fn handle_ioctl(fd: i32, req: u64, arg: u64) -> i64 {
    // All TTY and DRM ioctls on non-DRM fds route here.
    // Delegate to the main DRM ioctl handler — it handles TTY ioctls too.
    crate::drm::drm_ioctl(fd, req, arg)
}
