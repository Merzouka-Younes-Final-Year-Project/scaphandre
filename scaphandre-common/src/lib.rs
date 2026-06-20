#![no_std]

#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CpuEventType {
    ActivationEvent = 0,
    IdleEvent = 1,
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct CpuStateEvent {
    pub event_type: CpuEventType,
}
