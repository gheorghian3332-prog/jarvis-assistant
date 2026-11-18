use crate::service::{CoreError, CoreResult};
use windows::Win32::{
    Foundation::HANDLE,
    System::{
        ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX},
        Threading::{GetCurrentProcess, GetProcessTimes},
    },
};
#[derive(Debug, Clone, Copy)]
pub struct SelfStat {
    pub private_bytes: usize,
    pub working_set: usize,
    pub cpu_100ns: u64,
}
pub fn sample() -> CoreResult<SelfStat> {
    unsafe {
        let process: HANDLE = GetCurrentProcess();
        let mut memory = PROCESS_MEMORY_COUNTERS_EX::default();
        GetProcessMemoryInfo(
            process,
            std::ptr::addr_of_mut!(memory).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .map_err(|error| CoreError::Service(error.to_string()))?;
        let mut creation = Default::default();
        let mut exit = Default::default();
        let mut kernel = Default::default();
        let mut user = Default::default();
        GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user)
            .map_err(|error| CoreError::Service(error.to_string()))?;
        let to_u64 = |time: windows::Win32::Foundation::FILETIME| {
            (u64::from(time.dwHighDateTime) << 32) | u64::from(time.dwLowDateTime)
        };
        Ok(SelfStat {
            private_bytes: memory.PrivateUsage,
            working_set: memory.WorkingSetSize,
            cpu_100ns: to_u64(kernel) + to_u64(user),
        })
    }
}
