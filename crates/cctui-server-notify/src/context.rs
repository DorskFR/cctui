use cctui_server_kernel::context::KernelContext;

use crate::hooks::{NotifyDispatch, NotifySessions};

/// What the notify routes need from the host beyond the kernel.
pub trait NotifyContext: KernelContext + NotifySessions + NotifyDispatch {}

impl<T: KernelContext + NotifySessions + NotifyDispatch> NotifyContext for T {}
