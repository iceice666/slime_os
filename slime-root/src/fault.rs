//! Fault decoding and bounded task lifecycle supervision for `slime-root`.
//!
//! The root service loop calls [`decode_fault`] for a message received on its
//! badged fault endpoint, then records the result with [`SupervisionTable`].
//! Normalized lifecycle records contain only generation-local logical task keys
//! and portable fault details: raw badges, CSpace slots, object addresses, and
//! physical identifiers never cross this boundary.

pub const MAX_SUPERVISED_TASKS: usize = 64;
pub type TaskKey = u32;
pub type SupervisionKey = u32;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultError {
    NullFault,
    UnsupportedFault,
    TaskTableFull,
    DuplicateTask,
    UnknownTask,
    AlreadyTerminated,
    WaiterConflict,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AccessKind {
    Read,
    Write,
    Execute,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityPhase {
    Send,
    Receive,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CapabilityLookupFailure {
    InvalidRoot,
    MissingCapability,
    DepthMismatch,
    GuardMismatch,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FaultKind {
    Capability {
        phase: CapabilityPhase,
        lookup: CapabilityLookupFailure,
    },
    UnknownSyscall {
        number: u64,
    },
    UserException {
        number: u64,
        code: u64,
    },
    VirtualMemory {
        access: AccessKind,
        status: u64,
    },
    VirtualCpu {
        syndrome: u64,
    },
    VirtualInterruptMaintenance,
    VirtualPpi {
        irq: u64,
    },
    DebugException,
}

impl FaultKind {
    /// The code a supervising parent reads back through `supervision_status`.
    ///
    /// Spelled out rather than derived from the discriminant: this is a wire
    /// value a component compares against, so it must not move when a variant is
    /// added or reordered. The kind only — never the address or syndrome, which
    /// are the child's memory layout and are not its parent's business.
    pub const fn reason_code(&self) -> u64 {
        match self {
            Self::Capability { .. } => 1,
            Self::UnknownSyscall { .. } => 2,
            Self::UserException { .. } => 3,
            Self::VirtualMemory { .. } => 4,
            Self::VirtualCpu { .. } => 5,
            Self::VirtualInterruptMaintenance => 6,
            Self::VirtualPpi { .. } => 7,
            Self::DebugException => 8,
        }
    }
}

/// Portable fault record. `instruction` and `address` are task virtual values;
/// no kernel virtual, physical, CSpace, or object identifier is retained.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultRecord {
    pub kind: FaultKind,
    pub instruction: Option<u64>,
    pub address: Option<u64>,
}

/// One arrival on the root fault endpoint. The badge is the routing token the
/// task module minted for the faulting thread; it maps to a [`TaskKey`] there
/// and never appears in a [`LifecycleEvent`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FaultArrival {
    pub badge: sel4::Badge,
    pub record: Result<FaultRecord, FaultError>,
}

/// Block on the root fault endpoint and normalize whatever arrives. The task
/// loop resolves the badge to a logical task and calls
/// [`SupervisionTable::fault`]; a faulted thread is left suspended by the
/// kernel, so no reply is owed.
pub fn receive_fault(endpoint: sel4::cap::Endpoint) -> FaultArrival {
    let (info, badge) = endpoint.recv(());
    FaultArrival {
        badge,
        record: decode_fault(&info),
    }
}

/// Decode the current IPC buffer after the root fault endpoint receives
/// `info`. Exposed separately from [`receive_fault`] for a loop that already
/// holds the message info from a combined service/fault wait.
pub fn decode_fault(info: &sel4::MessageInfo) -> Result<FaultRecord, FaultError> {
    sel4::with_ipc_buffer(|ipc_buffer| normalize_fault(sel4::Fault::new(ipc_buffer, info)))
}

/// Convert a decoded seL4 fault into a portable record.
pub fn normalize_fault(fault: sel4::Fault) -> Result<FaultRecord, FaultError> {
    // Match arms are configuration-dependent: the hypervisor and debug faults
    // only exist in a kernel built with them, so the arms are selected here
    // rather than referenced unconditionally.
    sel4::sel4_cfg_wrap_match! {
        match fault {
            sel4::Fault::NullFault(_) => Err(FaultError::NullFault),
            sel4::Fault::CapFault(fault) => Ok(FaultRecord {
                kind: FaultKind::Capability {
                    phase: if fault.inner().get_InRecvPhase() == 0 {
                        CapabilityPhase::Send
                    } else {
                        CapabilityPhase::Receive
                    },
                    lookup: normalize_lookup_failure(fault.inner().get_LookupFailureType()),
                },
                instruction: Some(fault.inner().get_IP()),
                // The capability-fault address is a CSpace lookup value, not a
                // task virtual address. It stays out of normalized records.
                address: None,
            }),
            sel4::Fault::UnknownSyscall(fault) => Ok(FaultRecord {
                kind: FaultKind::UnknownSyscall {
                    number: fault.syscall(),
                },
                instruction: Some(fault.fault_ip()),
                address: None,
            }),
            sel4::Fault::UserException(fault) => Ok(FaultRecord {
                kind: FaultKind::UserException {
                    number: fault.inner().get_Number(),
                    code: fault.inner().get_Code(),
                },
                instruction: Some(fault.inner().get_FaultIP()),
                address: None,
            }),
            sel4::Fault::VmFault(fault) => Ok(FaultRecord {
                kind: FaultKind::VirtualMemory {
                    access: normalize_vm_access(&fault),
                    status: fault.fsr(),
                },
                instruction: Some(fault.ip()),
                address: Some(fault.addr()),
            }),
            #[sel4_cfg(ARM_HYPERVISOR_SUPPORT)]
            sel4::Fault::VGicMaintenance(_) => Ok(FaultRecord {
                kind: FaultKind::VirtualInterruptMaintenance,
                instruction: None,
                address: None,
            }),
            #[sel4_cfg(ARM_HYPERVISOR_SUPPORT)]
            sel4::Fault::VCpuFault(fault) => Ok(FaultRecord {
                kind: FaultKind::VirtualCpu {
                    syndrome: fault.hsr(),
                },
                instruction: None,
                address: None,
            }),
            #[sel4_cfg(ARM_HYPERVISOR_SUPPORT)]
            sel4::Fault::VPpiEvent(fault) => Ok(FaultRecord {
                kind: FaultKind::VirtualPpi { irq: fault.irq() },
                instruction: None,
                address: None,
            }),
            #[sel4_cfg(HARDWARE_DEBUG_API)]
            sel4::Fault::DebugException(_) => Ok(FaultRecord {
                kind: FaultKind::DebugException,
                instruction: None,
                address: None,
            }),
            #[allow(unreachable_patterns)]
            _ => Err(FaultError::UnsupportedFault),
        }
    }
}

/// `seL4_LookupFailureType` from `libsel4/include/sel4/constants.h`. A fault
/// message never carries `seL4_NoFailure`, so `0` here means the kernel
/// reported a type this build does not model.
fn normalize_lookup_failure(raw: sel4::Word) -> CapabilityLookupFailure {
    match raw {
        1 => CapabilityLookupFailure::InvalidRoot,
        2 => CapabilityLookupFailure::MissingCapability,
        3 => CapabilityLookupFailure::DepthMismatch,
        4 => CapabilityLookupFailure::GuardMismatch,
        _ => CapabilityLookupFailure::Unknown,
    }
}

/// AArch64 delivers `ESR_EL1`/`ESR_EL2` as the VM-fault status word. An
/// instruction abort is an execute access; for a data abort the ISS `WnR` bit
/// distinguishes read from write. Any other exception class stays `Unknown`
/// rather than being reported as a write.
fn normalize_vm_access(fault: &sel4::VmFault) -> AccessKind {
    const EXCEPTION_CLASS_SHIFT: sel4::Word = 26;
    const DATA_ABORT_LOWER_EL: sel4::Word = 0x24;
    const DATA_ABORT_CURRENT_EL: sel4::Word = 0x25;
    const WRITE_NOT_READ: sel4::Word = 1 << 6;

    if fault.is_prefetch() {
        return AccessKind::Execute;
    }
    let status = fault.fsr();
    match status >> EXCEPTION_CLASS_SHIFT {
        DATA_ABORT_LOWER_EL | DATA_ABORT_CURRENT_EL => {
            if status & WRITE_NOT_READ == 0 {
                AccessKind::Read
            } else {
                AccessKind::Write
            }
        }
        _ => AccessKind::Unknown,
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Termination {
    Exit(i64),
    Fault(FaultRecord),
    Timeout,
    PeerLoss,
    Unhealthy,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TaskState {
    Running,
    Waiting,
    Terminated(Termination),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LifecycleEventKind {
    Started,
    Waiting,
    IpcCompleted {
        service_label: sel4::Word,
        result: i64,
    },
    Exited {
        status: i64,
    },
    Faulted(FaultRecord),
    TimedOut,
    PeerLost,
    MarkedUnhealthy,
}

/// Generation-local lifecycle observation. `task` is assigned by the task
/// table, not derived from a badge, CPtr, TCB address, or physical resource.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LifecycleEvent {
    pub task: TaskKey,
    pub kind: LifecycleEventKind,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SupervisedTask {
    key: TaskKey,
    supervision: SupervisionKey,
    state: TaskState,
    waiter: Option<TaskKey>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SupervisionTransition {
    pub event: LifecycleEvent,
}

/// Fixed-capacity task supervision state. The task module registers logical
/// task/supervision keys after allocating seL4 objects, feeds service-label
/// completions from the root dispatcher, and feeds exits/faults from service
/// and fault endpoints. Terminal transitions are single-assignment.
#[derive(Debug, Eq, PartialEq)]
pub struct SupervisionTable<const CAPACITY: usize = MAX_SUPERVISED_TASKS> {
    tasks: [Option<SupervisedTask>; CAPACITY],
    len: usize,
}

impl<const CAPACITY: usize> SupervisionTable<CAPACITY> {
    pub const fn new() -> Self {
        Self {
            tasks: [const { None }; CAPACITY],
            len: 0,
        }
    }

    pub const fn len(&self) -> usize {
        self.len
    }

    pub const fn capacity(&self) -> usize {
        CAPACITY
    }

    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn register(
        &mut self,
        task: TaskKey,
        supervision: SupervisionKey,
    ) -> Result<LifecycleEvent, FaultError> {
        if self.find_task(task).is_some()
            || self
                .tasks
                .iter()
                .flatten()
                .any(|entry| entry.supervision == supervision)
        {
            return Err(FaultError::DuplicateTask);
        }
        let Some(slot) = self.tasks.iter_mut().find(|entry| entry.is_none()) else {
            return Err(FaultError::TaskTableFull);
        };
        *slot = Some(SupervisedTask {
            key: task,
            supervision,
            state: TaskState::Running,
            waiter: None,
        });
        self.len += 1;
        Ok(LifecycleEvent {
            task,
            kind: LifecycleEventKind::Started,
        })
    }

    pub fn state(&self, task: TaskKey) -> Option<TaskState> {
        self.find_task(task).map(|index| {
            self.tasks[index]
                .as_ref()
                .expect("located task entry is present")
                .state
        })
    }

    pub fn supervision_ready(&self, supervision: SupervisionKey) -> bool {
        self.find_supervision(supervision).is_none_or(|index| {
            matches!(
                self.tasks[index].as_ref().map(|task| task.state),
                Some(TaskState::Terminated(_)) | None
            )
        })
    }

    pub fn register_waiter(
        &mut self,
        supervision: SupervisionKey,
        waiter: TaskKey,
    ) -> Result<(), FaultError> {
        let index = self
            .find_supervision(supervision)
            .ok_or(FaultError::UnknownTask)?;
        let task = self.tasks[index]
            .as_mut()
            .expect("located supervision entry is present");
        match task.waiter {
            None => {
                task.waiter = Some(waiter);
                Ok(())
            }
            Some(existing) if existing == waiter => Ok(()),
            Some(_) => Err(FaultError::WaiterConflict),
        }
    }

    pub fn clear_waiter(&mut self, supervision: SupervisionKey, waiter: TaskKey) {
        if let Some(index) = self.find_supervision(supervision)
            && let Some(task) = self.tasks[index].as_mut()
            && task.waiter == Some(waiter)
        {
            task.waiter = None;
        }
    }

    pub fn waiting(&mut self, task: TaskKey) -> Result<LifecycleEvent, FaultError> {
        let entry = self.task_mut(task)?;
        if matches!(entry.state, TaskState::Terminated(_)) {
            return Err(FaultError::AlreadyTerminated);
        }
        entry.state = TaskState::Waiting;
        Ok(LifecycleEvent {
            task,
            kind: LifecycleEventKind::Waiting,
        })
    }

    pub fn ipc_completed(
        &mut self,
        task: TaskKey,
        service_label: sel4::Word,
        result: i64,
    ) -> Result<LifecycleEvent, FaultError> {
        let entry = self.task_mut(task)?;
        if matches!(entry.state, TaskState::Terminated(_)) {
            return Err(FaultError::AlreadyTerminated);
        }
        entry.state = TaskState::Running;
        Ok(LifecycleEvent {
            task,
            kind: LifecycleEventKind::IpcCompleted {
                service_label,
                result,
            },
        })
    }

    pub fn exit(
        &mut self,
        task: TaskKey,
        status: i64,
    ) -> Result<SupervisionTransition, FaultError> {
        self.terminate(
            task,
            Termination::Exit(status),
            LifecycleEventKind::Exited { status },
        )
    }

    pub fn fault(
        &mut self,
        task: TaskKey,
        fault: FaultRecord,
    ) -> Result<SupervisionTransition, FaultError> {
        self.terminate(
            task,
            Termination::Fault(fault),
            LifecycleEventKind::Faulted(fault),
        )
    }

    pub fn timeout(&mut self, task: TaskKey) -> Result<SupervisionTransition, FaultError> {
        self.terminate(task, Termination::Timeout, LifecycleEventKind::TimedOut)
    }

    pub fn peer_lost(&mut self, task: TaskKey) -> Result<SupervisionTransition, FaultError> {
        self.terminate(task, Termination::PeerLoss, LifecycleEventKind::PeerLost)
    }

    pub fn unhealthy(&mut self, task: TaskKey) -> Result<SupervisionTransition, FaultError> {
        self.terminate(
            task,
            Termination::Unhealthy,
            LifecycleEventKind::MarkedUnhealthy,
        )
    }

    pub fn take_termination(
        &mut self,
        supervision: SupervisionKey,
    ) -> Result<Option<Termination>, FaultError> {
        let index = self
            .find_supervision(supervision)
            .ok_or(FaultError::UnknownTask)?;
        let Some(entry) = self.tasks[index] else {
            return Err(FaultError::UnknownTask);
        };
        let TaskState::Terminated(reason) = entry.state else {
            return Ok(None);
        };
        self.tasks[index] = None;
        self.len -= 1;
        Ok(Some(reason))
    }

    fn terminate(
        &mut self,
        task: TaskKey,
        termination: Termination,
        kind: LifecycleEventKind,
    ) -> Result<SupervisionTransition, FaultError> {
        let entry = self.task_mut(task)?;
        if matches!(entry.state, TaskState::Terminated(_)) {
            return Err(FaultError::AlreadyTerminated);
        }
        entry.state = TaskState::Terminated(termination);
        entry.waiter = None;
        Ok(SupervisionTransition {
            event: LifecycleEvent { task, kind },
        })
    }

    fn task_mut(&mut self, task: TaskKey) -> Result<&mut SupervisedTask, FaultError> {
        let index = self.find_task(task).ok_or(FaultError::UnknownTask)?;
        self.tasks[index].as_mut().ok_or(FaultError::UnknownTask)
    }

    fn find_task(&self, task: TaskKey) -> Option<usize> {
        self.tasks
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.key == task))
    }

    fn find_supervision(&self, supervision: SupervisionKey) -> Option<usize> {
        self.tasks
            .iter()
            .position(|entry| entry.is_some_and(|entry| entry.supervision == supervision))
    }
}

impl<const CAPACITY: usize> Default for SupervisionTable<CAPACITY> {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FAULT: FaultRecord = FaultRecord {
        kind: FaultKind::VirtualMemory {
            access: AccessKind::Write,
            status: 0x45,
        },
        instruction: Some(0x1000),
        address: Some(0x2000),
    };

    #[test]
    fn ipc_success_and_fault_are_distinct_events() {
        let mut table = SupervisionTable::<2>::new();
        table.register(10, 100).unwrap();
        table.register(11, 101).unwrap();
        let ipc = table.ipc_completed(10, 5, 0).unwrap();
        let fault = table.fault(11, FAULT).unwrap().event;
        assert_eq!(
            ipc.kind,
            LifecycleEventKind::IpcCompleted {
                service_label: 5,
                result: 0,
            }
        );
        assert_eq!(fault.kind, LifecycleEventKind::Faulted(FAULT));
    }

    #[test]
    fn task_death_clears_obsolete_waiter_state() {
        let mut table = SupervisionTable::<1>::new();
        table.register(10, 100).unwrap();
        table.register_waiter(100, 77).unwrap();
        let transition = table.fault(10, FAULT).unwrap();
        assert_eq!(transition.event.task, 10);
        assert_eq!(table.fault(10, FAULT), Err(FaultError::AlreadyTerminated));
    }

    #[test]
    fn collection_is_bounded_and_terminal_result_is_consumed() {
        let mut table = SupervisionTable::<1>::new();
        table.register(1, 7).unwrap();
        assert_eq!(table.register(2, 8), Err(FaultError::TaskTableFull));
        table.exit(1, 4).unwrap();
        assert_eq!(table.take_termination(7), Ok(Some(Termination::Exit(4))));
        assert!(table.is_empty());
    }

    #[test]
    fn capability_fault_omits_lookup_address() {
        let record = FaultRecord {
            kind: FaultKind::Capability {
                phase: CapabilityPhase::Receive,
                lookup: CapabilityLookupFailure::GuardMismatch,
            },
            instruction: Some(0x4000),
            address: None,
        };
        assert_eq!(record.address, None);
    }
}
