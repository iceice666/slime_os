use super::*;

use core::ptr;
use slime_root::shared_buffer;

mod state;
use state::*;

mod console_runtime;
pub(super) mod platform;

use console_runtime::{ConsoleTables, declared_capability, input_script, start_console_dispatcher};
// Matches the sole caller's cfg in `main.rs`: the fixture image asserts the
// root's two-fixture proof path rather than launching a generation graph, so it
// never admits an IO-resource budget and never probes for userspace authority.
#[cfg(all(not(slime_boot_selector), not(slime_root_fixture)))]
pub(super) use platform::probe_authority_devices;

#[derive(Clone, Copy)]
pub(super) struct RootEndpoints {
    pub(super) service: sel4::cap::Endpoint,
    pub(super) console: sel4::cap::Endpoint,
}

pub(super) struct RuntimeDevices<'a> {
    pub(super) timer: &'a mut PhysicalTimerAdapter,
    #[cfg(slime_boot_selector)]
    pub(super) boot_blocks: &'a mut device::BlockDevices,
    pub(super) input: Option<device::TerminalInput>,
    pub(super) io_authority: &'a mut platform::AuthorityInventory,
}

pub(super) fn launch_instance_graph(
    generation: &Generation<'_>,
    admission: &Admission,
    bootinfo: &sel4::BootInfo,
    allocator: &mut ObjectAllocator,
    scratch: &ScratchPage,
    endpoints: RootEndpoints,
    devices: RuntimeDevices<'_>,
    #[cfg(slime_boot_selector)] boot_runtime: &mut boot_selector::BootRuntime,
) {
    let RuntimeDevices {
        timer,
        #[cfg(slime_boot_selector)]
        boot_blocks,
        input,
        io_authority,
    } = devices;
    // These three are ~488 KiB together and live in `.bss` rather than in this
    // frame; see the comment above `LAUNCH_TASKS`. As locals they subtracted
    // `0x7a000` from `sp` in one prologue step, overrunning the 1 MiB stack.
    let (tasks, windows, launched_instances) = init_launch_tables();
    let peers = unsafe { &mut *ptr::addr_of_mut!(PEER_ENDPOINTS) };
    let aligned = unsafe { &mut *ptr::addr_of_mut!(ELF_SCRATCH) };
    let mut launched = 0;
    // C10.2's declared private-memory budget, resolved once. `Admission::admit`
    // has already refused a malformed or over-committed one, so anything
    // reaching here is a budget this root can honour in full.
    let private_budget = match crate::generation::private_memory_budget_object(generation) {
        Some(Ok(budget)) => Some(budget),
        Some(Err(error)) => {
            fatal!("SLIME_MEM FAIL admitted budget will not decode: {error:?}")
        }
        None => None,
    };
    sel4::debug_println!(
        "SLIME_MEM budget holders={} declared={}",
        private_budget
            .as_ref()
            .map_or(0, PrivateMemoryBudget::holder_count),
        private_budget.is_some() as u8,
    );
    let clock_authority = match generation::clock_authority_object(generation) {
        Some(Ok(authority)) => Some(authority),
        Some(Err(error)) => {
            fatal!("SLIME_CLOCK FAIL admitted authority will not decode: {error:?}")
        }
        None => None,
    };
    sel4::debug_println!(
        "SLIME_CLOCK authority holders={} timer_quota={}",
        clock_authority
            .as_ref()
            .map_or(0, |authority| authority.holder_count()),
        clock_authority
            .as_ref()
            .map_or(0, |authority| authority.timer_quota()),
    );
    let wait_sources = match wait_set::source_object(generation) {
        Some(Ok(sources)) => Some(sources),
        Some(Err(error)) => {
            fatal!("SLIME_WAIT FAIL admitted sources will not decode: {error:?}")
        }
        None => None,
    };
    sel4::debug_println!(
        "SLIME_WAIT sources declared={} resource={}",
        wait_sources
            .as_ref()
            .map_or(0, |sources| sources.entry_count()),
        wait_sources.is_some() as u8,
    );
    let scheduling_policy = match generation::scheduling_class_object(generation) {
        Some(Ok(policy)) => Some(policy),
        Some(Err(error)) => {
            fatal!("SLIME_SCHED FAIL admitted policy will not decode: {error:?}")
        }
        None => None,
    };
    let lifecycle_policy = match lifecycle::policy_object(generation) {
        Some(Ok(policy)) => Some(policy),
        Some(Err(error)) => {
            fatal!("SLIME_LIFECYCLE FAIL admitted policy will not decode: {error:?}")
        }
        None => None,
    };
    let io_budget = match generation::io_resource_budget_object(generation) {
        Some(Ok(budget)) => Some(budget),
        Some(Err(error)) => fatal!("SLIME_IO FAIL admitted budget will not decode: {error:?}"),
        None => None,
    };
    let mut io_service = services::IoResourceService::new();

    for instance_index in 0..generation.instance_count() {
        let instance = match generation.instance(instance_index) {
            Ok(instance) => instance,
            Err(error) => fatal!("SLIME_GRAPH FAIL instance rejected: {error:?}"),
        };
        if !instance.is_root_autostart() {
            continue;
        }
        let executable = match generation.executable(instance.executable) {
            Ok(executable) => executable,
            Err(error) => fatal!("SLIME_GRAPH FAIL executable rejected: {error:?}"),
        };
        let Some(plan) = admission.executable_plan(instance.executable) else {
            fatal!(
                "SLIME_GRAPH FAIL executable {} was not admitted",
                executable.name
            )
        };
        if !plan.format.is_loadable() {
            fatal!(
                "SLIME_GRAPH FAIL root instance {} executable {} is not loadable",
                instance.name,
                executable.name
            )
        }
        let object = match generation.object(executable.object) {
            Ok(object) => object,
            Err(error) => fatal!("SLIME_GRAPH FAIL object rejected: {error:?}"),
        };
        let profile = match boot_contracts::target_profile::TargetProfile::by_name(TARGET_PROFILE) {
            Ok(profile) => profile,
            Err(error) => fatal!("SLIME_GRAPH FAIL profile unavailable: {error:?}"),
        };
        let elf = match boot_contracts::component_image::admit_elf(object.bytes, profile) {
            Ok(elf) => elf,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL executable {} refused: {error:?}",
                executable.name
            ),
        };
        let elf = match aligned.hold(elf) {
            Ok(elf) => elf,
            Err(len) => fatal!(
                "SLIME_GRAPH FAIL executable {} is {len} bytes, over the load bound",
                executable.name
            ),
        };
        let image = match ChildImage::parse(elf) {
            Ok(image) => image,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL executable {} image rejected: {error:?}",
                executable.name
            ),
        };
        let authority = match bound_authority(generation, instance) {
            Ok(authority) => authority,
            Err(error) => fatal!("SLIME_GRAPH FAIL binding authority rejected: {error:?}"),
        };
        // The child's CSpace is exactly as large as the admitted plan says its
        // declared authority needs, not a compiled-in shell.
        let cspace_size_bits = match generation.instance_cspace_size_bits(instance_index) {
            Ok(Some(bits)) => bits as usize,
            Ok(None) => fatal!(
                "SLIME_GRAPH FAIL instance {} has no planned CSpace",
                instance.name
            ),
            Err(error) => fatal!("SLIME_GRAPH FAIL CSpace plan rejected: {error:?}"),
        };
        // Priority likewise. An instance that declares none resolves to the
        // root's default, which is what every child ran at before the plan's
        // `ScheduleRecord` was consulted at all (B48).
        let declared_priority = match generation.instance_priority(instance_index) {
            Ok(Some(priority)) => sel4::Word::from(priority),
            Ok(None) => task::CHILD_PRIORITY,
            Err(error) => fatal!("SLIME_GRAPH FAIL schedule plan rejected: {error:?}"),
        };
        // Its own record rather than a field on `staged`: the priority a
        // thread runs at is not observable from anything else in the
        // transcript, and a declaration nothing can check is indistinguishable
        // from the constant it replaced (B48).
        sel4::debug_println!(
            "SLIME_GRAPH schedule instance={} priority={declared_priority} default={}",
            instance.name,
            task::CHILD_PRIORITY,
        );
        // Threads the plan declares for this instance (B47). One unless the
        // manifest asked for more; the root builds exactly this many TCBs, so
        // a declared thread that never runs would be visible here as a count
        // the transcript disagrees with.
        let declared_threads = match generation.instance_threads(instance_index) {
            Ok(Some(threads)) => threads,
            Ok(None) => 1,
            Err(error) => fatal!("SLIME_GRAPH FAIL thread plan rejected: {error:?}"),
        };
        sel4::debug_println!(
            "SLIME_GRAPH threads instance={} count={declared_threads}",
            instance.name,
        );
        // Each worker's own declared priority (B48). Resolved here rather than
        // in `task::create` so the transcript records what the plan asked for,
        // the same way the main thread's priority is recorded above.
        let mut declared_worker_priorities = [declared_priority; child_vspace::MAX_CHILD_THREADS];
        for (thread_index, slot) in declared_worker_priorities
            .iter_mut()
            .enumerate()
            .take(declared_threads)
            .skip(1)
        {
            let resolved = match generation.thread_priority(instance_index, thread_index) {
                Ok(Some(priority)) => sel4::Word::from(priority),
                Ok(None) => declared_priority,
                Err(error) => fatal!("SLIME_GRAPH FAIL thread schedule rejected: {error:?}"),
            };
            *slot = resolved;
            sel4::debug_println!(
                "SLIME_GRAPH schedule instance={} thread={thread_index} priority={resolved}",
                instance.name,
            );
        }
        // The child's own TCB and fault endpoint go where the plan declared
        // them. A plan that omits either leaves the root nowhere to install
        // authority the child needs, so it is refused rather than defaulted.
        let child_slots = match generation.instance_child_slots(instance_index) {
            Ok(Some(boot_contracts::generation::ChildSlotPlan {
                service: Some(service),
                console: Some(console),
                tcb: Some(tcb),
                fault: Some(fault),
            })) => match (task::ChildSlots {
                service: service as sel4::CPtrBits,
                console: console as sel4::CPtrBits,
                tcb: tcb as sel4::CPtrBits,
                fault: fault as sel4::CPtrBits,
            })
            .validate()
            {
                Ok(slots) => slots,
                Err(error) => fatal!(
                    "SLIME_GRAPH FAIL instance {} declares an unusable child layout: {error:?}",
                    instance.name
                ),
            },
            Ok(_) => fatal!(
                "SLIME_GRAPH FAIL instance {} has no planned service, console, TCB, or fault slot",
                instance.name
            ),
            Err(error) => fatal!("SLIME_GRAPH FAIL child slot plan rejected: {error:?}"),
        };
        let id = match tasks.create(
            allocator,
            &image,
            endpoints.service,
            endpoints.console,
            authority,
            Supervision::SelfManaged,
            sel4::init_thread::slot::VSPACE.cap(),
            scratch,
            sel4::init_thread::slot::ASID_POOL.cap(),
            None,
            Some(instance.executable),
            Some(instance_index),
            // Only the bootstrap instance composes a boot graph, so only it is
            // told which one. Every other instance starts with zero.
            if instance_index == generation.bootstrap() {
                generation.boot_action.id()
            } else {
                0
            },
            cspace_size_bits,
            child_slots,
            declared_priority,
            declared_threads,
            declared_worker_priorities,
            // The quota this generation declares for this component (C10.2).
            // Zero for a component the budget does not name, which is the same
            // deny-by-default answer an absent `sharedBufferBudget` holder
            // gets: authority is never ambient, so omission means "grows
            // nothing" rather than "gets a small default".
            //
            // Resolved before `create` rather than installed after it, unlike
            // the shared-buffer quota: `create` feeds this into the arena plan,
            // because an arena is fixed at construction and never grows, so a
            // ceiling whose frames the arena has no room for would be one the
            // task could never reach.
            declared_private_memory_pages(private_budget.as_ref(), instance.name),
        ) {
            Ok(id) => id,
            Err(error) => fatal!(
                "SLIME_GRAPH FAIL instance {} construction failed: {error:?}",
                instance.name
            ),
        };
        let Some(task) = tasks.get(id) else {
            fatal!("SLIME_GRAPH FAIL constructed task {} is missing", id.0)
        };
        // One window per thread (B47): each stages its own payloads, and a
        // thread whose window was never declared is refused at bind and cannot
        // receive anything.
        for (thread, pages) in task
            .vspace
            .pages
            .iter()
            .take(task.vspace.threads)
            .enumerate()
        {
            if let Err(error) = windows.declare(
                id,
                thread,
                pages.transfer_window_addr,
                pages.transfer_window,
                pages.transfer_window_alias,
            ) {
                fatal!("SLIME_GRAPH FAIL window declaration rejected: {error:?}")
            }
        }
        let transfer_window_addr = task.vspace.main().transfer_window_addr;
        let frames_mapped = task.vspace.frames_mapped;
        let tables_mapped = task.vspace.tables_mapped;
        let entry = task.entry;
        let Some(table) = tasks.authority_mut(id) else {
            fatal!(
                "SLIME_GRAPH FAIL capability table unavailable for task {}",
                id.0
            )
        };

        for binding_index in 0..instance.binding_count() {
            let binding = match generation.binding(instance, binding_index) {
                Ok(binding) => binding,
                Err(error) => fatal!(
                    "SLIME_GRAPH FAIL instance {} binding rejected: {error:?}",
                    instance.name
                ),
            };
            let grant = match generation.grant(binding.grant) {
                Ok(grant) => grant,
                Err(error) => fatal!("SLIME_GRAPH FAIL bound grant rejected: {error:?}"),
            };
            if grant.capability_kind == CapabilityKind::Endpoint {
                continue;
            }
            if !generation.grant_applies_to_instance(grant, instance_index) {
                fatal!(
                    "SLIME_GRAPH FAIL binding {} is unrelated to instance {}",
                    grant.name,
                    instance.name
                )
            }
            let capability = if grant.capability_kind == CapabilityKind::Executable {
                let GrantEndpoint::Executable(executable) = grant.target else {
                    fatal!(
                        "SLIME_GRAPH FAIL binding {} executable target rejected",
                        grant.name
                    )
                };
                graph::CapabilityEntry::executable(executable, grant.rights)
            } else {
                // Every kind this path still constructs is either singular or
                // carries its per-device identity in the IO-resource authority
                // table rather than in a launch-order ordinal, so the resource
                // argument is 0. B90 deleted the one ordinal that was not: the
                // `Block` counter, whose value no operation ever read.
                declared_capability(grant.capability_kind, 0, grant.rights)
            };
            let Some(capability) = capability else {
                fatal!(
                    "SLIME_GRAPH FAIL binding {} carries invalid typed authority",
                    grant.name
                )
            };
            let slot = match u32::try_from(binding.slot) {
                Ok(slot) => slot,
                Err(_) => fatal!(
                    "SLIME_GRAPH FAIL binding {} slot is out of range",
                    grant.name
                ),
            };
            if let Err(error) = table.install(slot, capability) {
                fatal!(
                    "SLIME_GRAPH FAIL binding {} slot={slot} rejected: {error:?}",
                    grant.name
                )
            }
            // A factory is authority to mint objects, so where it lands is the
            // difference between a component reaching its own declared factory
            // and reaching none. The boot layout names the slot and this
            // records that the root honoured it.
            if matches!(capability, graph::CapabilityEntry::BufferFactory(_)) {
                sel4::debug_println!(
                    "SLIME_GRAPH factory placed task={} component={} slot={slot} kind={}",
                    id.0,
                    instance.name,
                    capability.kind_name(),
                );
            }
        }
        if let Some(quota) = io_budget.as_ref().and_then(|budget| {
            budget.quota_for(&boot_contracts::io_resource::driver_identity(instance.name))
        }) {
            let ordinal = quota.device as usize;
            let shared = io_authority.device(ordinal).is_some_and(|device| {
                (0..io_authority.len()).any(|other_ordinal| {
                    other_ordinal != ordinal
                        && io_authority
                            .device(other_ordinal)
                            .is_some_and(|other| other.region == device.region)
                })
            });
            if let Err(error) = services::install_driver(
                &mut io_service,
                slime_root::io_resource::DriverId(u64::from(id.0)),
                instance_index,
                quota,
                shared,
            ) {
                fatal!(
                    "SLIME_IO FAIL quota install task={} instance={} error={error:?}",
                    id.0,
                    instance.name
                )
            }
            sel4::debug_println!(
                "SLIME_IO quota task={} instance={} devices={} shared_granule={}",
                id.0,
                instance.name,
                io_authority.len(),
                shared as u8
            );
        }
        if let Err(error) = launched_instances.record(instance_index, instance.executable, id) {
            fatal!("SLIME_GRAPH FAIL instance mapping rejected: {error:?}")
        }
        sel4::debug_println!(
            "SLIME_GRAPH staged task={} instance={} executable={} grants={} bindings={} window={:#x} frames={} tables={} entry={:#x}",
            id.0,
            instance.name,
            executable.name,
            authority.grants,
            instance.binding_count(),
            transfer_window_addr,
            frames_mapped,
            tables_mapped,
            entry,
        );
        launched += 1;
    }

    sel4::debug_println!(
        "SLIME_GRAPH staged instances={launched} root_autostart={} loadable_executables={} slimecm={} wrong_target={} unrecognized={}",
        admission.root_autostart_instances(generation).count(),
        admission.loadable,
        admission.slime_component_images,
        admission.wrong_target_images,
        admission.unrecognized_images,
    );
    let materialized = match peers.materialize(generation, launched_instances, allocator, tasks) {
        Ok(report) => report,
        Err(error) => fatal!("SLIME_GRAPH FAIL endpoint materialization rejected: {error:?}"),
    };
    sel4::debug_println!(
        "SLIME_GRAPH peer endpoints created={} grants={} installed={}",
        peers.len(),
        materialized.grants,
        materialized.installed,
    );
    let notifications = unsafe { &mut *ptr::addr_of_mut!(NOTIFICATIONS) };
    let mut notification_report = match notifications.materialize(generation, allocator) {
        Ok(report) => report,
        Err(error) => fatal!("SLIME_GRAPH FAIL notification materialization rejected: {error:?}"),
    };
    for launched in launched_instances.iter() {
        let Some(task) = tasks.get(launched.task) else {
            fatal!(
                "SLIME_GRAPH FAIL launched task {} is missing",
                launched.task.0
            )
        };
        let installed = match notifications.install_instance(
            generation,
            launched.instance,
            launched.task,
            allocator,
            task.cleanup.arena,
            task.cnode,
            task.cnode_size_bits,
        ) {
            Ok(installed) => installed,
            Err(error) => fatal!("SLIME_GRAPH FAIL notification install rejected: {error:?}"),
        };
        notification_report.bindings += installed;
        // C8.13.3: each install filled a slot the generation declared, so it
        // belongs to the holder's declared-space count -- the space
        // `capabilitySlots` budgets. Credited rather than censused because
        // every install here is the root's own.
        if let Some(task) = tasks.get_mut(launched.task) {
            task.cspace.installed(installed as u32);
        }
    }
    sel4::debug_println!(
        "SLIME_GRAPH notifications created={} bindings={}",
        notification_report.created,
        notification_report.bindings,
    );
    // This generation creates one clock service instance. The static lives
    // across the process lifetime, but a boot reaches this path once; resetting
    // here makes that lifetime explicit and prevents stale authority if the
    // launch path ever becomes restartable.
    unsafe { *ptr::addr_of_mut!(CLOCK_SERVICE) = clock::ClockService::new() };
    let clock_service = unsafe { &mut *ptr::addr_of_mut!(CLOCK_SERVICE) };
    for launched in launched_instances.iter() {
        let Some(task) = tasks.get(launched.task) else {
            fatal!(
                "SLIME_CLOCK FAIL launched task {} is missing",
                launched.task.0
            )
        };
        let authority = match clock_service.declare(
            clock_authority.as_ref(),
            generation,
            notifications,
            allocator,
            task.cleanup.arena,
            launched.task,
            launched.instance,
        ) {
            Ok(authority) => authority,
            Err(error) => fatal!(
                "SLIME_CLOCK FAIL authority install task={} error={error:?}",
                launched.task.0
            ),
        };
        sel4::debug_println!(
            "SLIME_CLOCK authority task={} instance={} flags={:#x} timers={} badge={:#x}",
            launched.task.0,
            generation
                .instance(launched.instance)
                .map_or("?", |instance| instance.name),
            authority.flags(),
            authority.timer_quota(),
            authority.timer_badge(),
        );
    }

    // C9.2's supervision-source delivery, declared per launched task on the same
    // rule as the clock authority above: every live task gets a row, including
    // one the resource names no source for, so the table answers about a live
    // task rather than about whether it was ever declared.
    unsafe { *ptr::addr_of_mut!(WAIT_SET_SERVICE) = wait_set::WaitSetService::new() };
    let wait_set_service = unsafe { &mut *ptr::addr_of_mut!(WAIT_SET_SERVICE) };
    for launched in launched_instances.iter() {
        let Some(task) = tasks.get(launched.task) else {
            fatal!(
                "SLIME_WAIT FAIL launched task {} is missing",
                launched.task.0
            )
        };
        let declared = match wait_set_service.declare(
            wait_sources.as_ref(),
            generation,
            notifications,
            allocator,
            task.cleanup.arena,
            launched.task,
            launched.instance,
        ) {
            Ok(declared) => declared,
            Err(error) => fatal!(
                "SLIME_WAIT FAIL source install task={} error={error:?}",
                launched.task.0
            ),
        };
        if declared != 0 {
            sel4::debug_println!(
                "SLIME_WAIT supervision task={} instance={} sources={declared}",
                launched.task.0,
                generation
                    .instance(launched.instance)
                    .map_or("?", |instance| instance.name),
            );
        }
    }

    // C9.3's declared class, recorded per launched task on the same rule as the
    // clock authority and wait sources above: every live task gets a row,
    // including one the policy names no class for, so the table answers about a
    // live task rather than about whether it was ever declared.
    //
    // The band's priority is *already* on each thread: it travels in the plan's
    // `ScheduleRecord`, which the builder wrote from this same resource, and
    // `TaskTable::create` applied it before this loop runs. What this install
    // adds is the root's own view — which class each task is at, so
    // `CLASS_READ` can answer and `CLASS_PROMOTE` can find a subject. Deriving
    // the priority twice is exactly the drift B71 closed, so the marker prints
    // the class beside the priority the schedule record already carries.
    unsafe { *ptr::addr_of_mut!(SCHEDULING_SERVICE) = scheduling::SchedulingService::new() };
    let scheduling_service = unsafe { &mut *ptr::addr_of_mut!(SCHEDULING_SERVICE) };
    for launched in launched_instances.iter() {
        let class = match scheduling_service.declare(
            scheduling_policy.as_ref(),
            generation,
            launched.task,
            launched.instance,
        ) {
            Ok(class) => class,
            Err(error) => fatal!(
                "SLIME_SCHED FAIL class install task={} error={error:?}",
                launched.task.0
            ),
        };
        if scheduling_policy.is_some() {
            sel4::debug_println!(
                "SLIME_SCHED class task={} instance={} class={} priority={} worker={} worker_priority={}",
                launched.task.0,
                generation
                    .instance(launched.instance)
                    .map_or("?", |instance| instance.name),
                class.name(),
                class.priority(),
                boot_contracts::scheduling_class::class_name(class.worker_class_id()),
                class.worker_priority(),
            );
        }
    }
    if let Some(policy) = scheduling_policy.as_ref() {
        sel4::debug_println!(
            "SLIME_SCHED policy bands={} instances={} promotions={} unnamed={}",
            policy.class_count(),
            policy.instance_count(),
            policy.promotion_count(),
            boot_contracts::scheduling_class::class_name(
                boot_contracts::scheduling_class::UNDECLARED_CLASS_ID
            ),
        );
        for index in 0..policy.class_count() {
            let Some(band) = policy.band(index) else {
                fatal!("SLIME_SCHED FAIL band {index} is missing")
            };
            sel4::debug_println!(
                "SLIME_SCHED band class={} priority={}",
                boot_contracts::scheduling_class::class_name(band.class_id),
                band.priority,
            );
        }
    }
    // C9.4's declared state, installed for every launched instance on the same
    // rule as its class above: recorded for every live task, including one the
    // policy names nothing for, so the table answers about a live task rather
    // than about whether it was ever declared.
    unsafe { *ptr::addr_of_mut!(LIFECYCLE_SERVICE) = lifecycle::LifecycleService::new() };
    let lifecycle_service = unsafe { &mut *ptr::addr_of_mut!(LIFECYCLE_SERVICE) };
    for launched in launched_instances.iter() {
        let state = match lifecycle_service.declare(
            lifecycle_policy.as_ref(),
            launched.task,
            launched.instance,
        ) {
            Ok(state) => state,
            Err(error) => fatal!(
                "SLIME_LIFECYCLE FAIL state install task={} error={error:?}",
                launched.task.0
            ),
        };
        if lifecycle_policy.is_some() {
            sel4::debug_println!(
                "SLIME_LIFECYCLE state task={} instance={} state={} attempts={}",
                launched.task.0,
                generation
                    .instance(launched.instance)
                    .map_or("?", |instance| instance.name),
                boot_contracts::lifecycle_policy::state_name(state),
                lifecycle_service.attempts_remaining(
                    lifecycle_policy.as_ref(),
                    launched.instance,
                    generation
                ),
            );
        }
    }
    if let Some(policy) = lifecycle_policy.as_ref() {
        // `admitted=` is the count *admission* resolved, printed beside the
        // count the resource decodes to. Two producers of one number, exactly as
        // C9.3's class marker is cross-checked against the `ScheduleRecord` the
        // builder wrote: admission walks every restart row and proves its subject
        // is owner-spawned, so a disagreement here means the policy the root
        // validated is not the policy it is about to install (B71's shape).
        let admitted = admission.lifecycle_restarts.unwrap_or(0);
        if admitted != policy.restart_count() {
            fatal!(
                "SLIME_LIFECYCLE FAIL admission counted {admitted} restart policies, resource declares {}",
                policy.restart_count()
            )
        }
        sel4::debug_println!(
            "SLIME_LIFECYCLE policy transitions={} restarts={} admitted={admitted} dependencies={} parameters={} initial={} terminal={}",
            policy.transition_count(),
            policy.restart_count(),
            policy.dependency_count(),
            policy.parameter_count(),
            boot_contracts::lifecycle_policy::state_name(policy.initial_state()),
            boot_contracts::lifecycle_policy::state_name(policy.terminal_state()),
        );
        for index in 0..policy.transition_count() {
            let Some(edge) = policy.transition(index) else {
                fatal!("SLIME_LIFECYCLE FAIL transition {index} is missing")
            };
            sel4::debug_println!(
                "SLIME_LIFECYCLE edge from={} to={}",
                boot_contracts::lifecycle_policy::state_name(edge.from_state),
                boot_contracts::lifecycle_policy::state_name(edge.to_state),
            );
        }
    }

    let bootstrap = launched_instances.task_for_instance(admission.bootstrap_instance);
    let table = bootstrap.and_then(|id| tasks.authority(id));
    sel4::debug_println!(
        "[layout] path={} slots={} max={}",
        generation
            .instance(admission.bootstrap_instance)
            .map_or("?", |instance| instance.name),
        table.map_or(0, |table| table.len()),
        graph::MAX_TASK_CAPS,
    );
    if let Some(table) = table {
        for (slot, capability) in table.slots() {
            let Some(capability) = capability else {
                continue;
            };
            sel4::debug_println!(
                "[layout] {slot} {} {} {:#x}",
                capability.kind_name(),
                resource_label(generation, capability),
                capability.rights_bits(),
            );
        }
    }
    sel4::debug_println!("[layout] end");

    let mut active = [false; MAX_TASKS];
    let mut activated = 0;
    while activated < launched {
        let before = activated;
        for launched_instance in launched_instances.iter() {
            if active[launched_instance.instance] {
                continue;
            }
            let instance = match generation.instance(launched_instance.instance) {
                Ok(instance) => instance,
                Err(error) => fatal!("SLIME_GRAPH FAIL activation instance rejected: {error:?}"),
            };
            let mut ready = true;
            for dependency_index in 0..instance.dependency_count() {
                let dependency = match generation.dependency(instance, dependency_index) {
                    Ok(dependency) => dependency,
                    Err(error) => fatal!("SLIME_GRAPH FAIL dependency rejected: {error:?}"),
                };
                let dependency_index = (0..generation.instance_count())
                    .find(|index| {
                        generation
                            .instance(*index)
                            .is_ok_and(|candidate| candidate.name == dependency.name)
                    })
                    .unwrap_or(usize::MAX);
                if dependency_index >= active.len()
                    || launched_instances
                        .task_for_instance(dependency_index)
                        .is_none()
                {
                    fatal!(
                        "SLIME_GRAPH FAIL instance {} has non-root dependency {}",
                        instance.name,
                        dependency.name
                    )
                }
                ready &= active[dependency_index];
            }
            if !ready {
                continue;
            }
            if let Err(error) = tasks.activate(launched_instance.task) {
                fatal!(
                    "SLIME_GRAPH FAIL activation failed instance={}: {error:?}",
                    instance.name
                )
            }
            active[launched_instance.instance] = true;
            activated += 1;
        }
        if activated == before {
            fatal!("SLIME_GRAPH FAIL root instance dependency barrier unsatisfied")
        }
    }
    sel4::debug_println!("SLIME_GRAPH activated instances={activated}");
    // No component request has been received yet: activation can queue a
    // synchronous IPC call, but only the service loop below can mutate the
    // ClockService scheduler or program its hardware deadline. This second
    // proof therefore establishes post-activation IRQ liveness before timer
    // ownership passes to the service loop.
    prove_timer(timer, "post-graph-start");

    let mut buffers = SharedBufferTable::new(GenerationEpoch(generation.number));
    let budget = shared_buffer_budget(generation);
    let mut budgeted = 0;
    for launched_instance in launched_instances.iter() {
        let instance = generation.instance(launched_instance.instance).unwrap();
        let quota = declared_quota(budget.as_ref(), instance.name);
        if quota != HolderQuota::DENY {
            budgeted += 1;
        }
        if let Err(error) =
            buffers.declare_quota(HolderId(u64::from(launched_instance.task.0)), quota)
        {
            fatal!(
                "SLIME_GRAPH FAIL quota rejected task={}: {error:?}",
                launched_instance.task.0
            )
        }
        // The same record the spawn path emits. The boot path declared its
        // quotas silently and printed only the aggregate, so a per-instance
        // ceiling could be wrong in the generation and invisible in the
        // transcript -- and a gate checking the declared ceilings against the
        // observed ones saw only the spawned children (B52).
        sel4::debug_println!(
            "SLIME_GRAPH quota task={} instance={} executable={} pages={} buffers={} mappings={} loans={}",
            launched_instance.task.0,
            instance.name,
            generation
                .executable(launched_instance.executable)
                .map_or("<unknown>", |record| record.name),
            quota.byte_pages,
            quota.buffer_count,
            quota.mapping_count,
            quota.loan_count,
        );
        // The private-memory ceiling actually installed on the task, read back
        // from the task record rather than from the budget (C10.2). Reading it
        // back is the point: the declared number and the live one are two
        // different facts, and a gate comparing the generation's declaration
        // against this line is what proves the declared quota *is* the ceiling
        // rather than something the root recomputed on its own.
        sel4::debug_println!(
            "SLIME_MEM quota task={} instance={} declared={} installed={} base={:#x}",
            launched_instance.task.0,
            instance.name,
            declared_private_memory_pages(private_budget.as_ref(), instance.name),
            tasks
                .get(launched_instance.task)
                .map_or(0, |task| task.private_memory.quota()),
            tasks
                .get(launched_instance.task)
                .map_or(0, |task| task.private_memory.base()),
        );
    }
    sel4::debug_println!(
        "SLIME_GRAPH quotas declared={} budgeted={budgeted} holders={}",
        launched_instances.len(),
        budget.as_ref().map_or(0, SharedBufferBudget::holder_count),
    );
    // The shared filesystem namespaces and their append-only interned scopes.
    let mut namespaces = directory::Namespaces::new();
    let mut scopes = directory::ScopeTable::new();

    // B41: the console dispatcher starts before the service loop, so console
    // traffic has a receiver for as long as any child can send. `windows`
    // outlives it — `serve_instance_graph` does not return.
    start_console_dispatcher(
        bootinfo,
        allocator,
        endpoints.console,
        ConsoleTables {
            windows,
            tasks,
            script: input_script(generation.number),
            input,
            namespaces: &mut namespaces,
            scopes: &scopes,
        },
    );

    serve_instance_graph(
        generation,
        launched_instances,
        endpoints.service,
        endpoints.console,
        tasks,
        windows,
        &mut buffers,
        clock_service,
        wait_set_service,
        scheduling_service,
        scheduling_policy.as_ref(),
        lifecycle_service,
        lifecycle_policy.as_ref(),
        timer,
        allocator,
        scratch,
        &mut io_service,
        io_authority,
        admission.fabric_capability_slots,
        &mut scopes,
        #[cfg(slime_boot_selector)]
        boot_blocks,
        #[cfg(slime_boot_selector)]
        boot_runtime,
    );
}

/// Component arrivals the graph service loop will handle before declaring the
/// graph wedged. Bound timer-Notification wakes are serviced independently and
/// do not consume this request-path progress ceiling.
///
/// Generous against what the declared components actually issue — each binds a
/// window, and spawn-service additionally runs a shared-buffer probe and spawns
/// two children — while still bounding a request livelock so it fails in seconds
/// rather than burning the gate's whole timeout.
///
/// component that blocks now costs two iterations where it cost one — the
/// `recv` that reports `WouldBlock` and the `wait` that parks.
///
/// The stream plane's nine tasks provision four route roles and broker seven
/// samples in **136** iterations. The **QoS** plane is far denser and is what
/// set this number: it drives a simulated clock through scheduled deadline,
/// lifespan, liveliness, and retry boundaries, and each boundary is a park/wake
/// cycle for the broker plus a sweep of every participant. At 512 it exhausted
/// the bound with `fabric-publisher`'s send still queued — diagnosed at length
/// as B28 and mistaken in turn for a lost wake, a scheduler fault, and an
/// always-ready park source before the cause turned out to be this constant.
/// Measured directly: **768 completes, 512 does not.** 2048 is that floor with
/// Raised for P5.4.3's dango plane, which is the densest composition
/// this port runs: a scripted console session is one round trip *per keystroke*
/// — 96 bytes of script — on top of four components' startup, every command's
/// profile resolution, and a spawn plus a supervised wait per launch. Measured
/// the same way B28 was: 2048 exhausted with the session still parked.
const MAX_GRAPH_ITERATIONS: usize = 32768;

mod services;
pub(super) use services::policy::private_memory_cause;
use services::{serve_instance_graph, spawn::resource_label};
