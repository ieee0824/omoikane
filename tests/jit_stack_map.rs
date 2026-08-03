//! Gate 4-1 integration contract for JIT frames, safepoints, and stack maps.

#![cfg(feature = "baseline-jit")]

use std::sync::Arc;

use boa_engine::jit::{
    ActiveJitFrame, FrameCaller, FrameMetadataError, JitFrameChain, JitFrameDescriptor,
    JitFrameDescriptorId, JitFrameHeader, JitPcTable, Safepoint, SafepointKind, StackMap,
    ValueLocation,
};

fn safepoint(
    machine_offset: u32,
    bytecode_offset: u32,
    kind: SafepointKind,
    live_values: impl IntoIterator<Item = ValueLocation>,
) -> Safepoint {
    Safepoint {
        machine_offset,
        bytecode_offset,
        kind,
        stack_map: StackMap::new(live_values),
    }
}

#[test]
fn nested_interpreter_and_jit_frames_resolve_exact_live_values() {
    let outer = Arc::new(
        JitFrameDescriptor::new(
            JitFrameDescriptorId(5341),
            64,
            32,
            3,
            [
                safepoint(
                    8,
                    12,
                    SafepointKind::Call,
                    [
                        ValueLocation::MachineRegister(2),
                        ValueLocation::StackSlot(-8),
                        ValueLocation::FrameRegister(0),
                        // Intentional duplicate: stack maps canonicalize live locations.
                        ValueLocation::FrameRegister(0),
                    ],
                ),
                safepoint(24, 28, SafepointKind::LoopBackedge, []),
            ],
        )
        .expect("valid outer descriptor"),
    );
    let inner = Arc::new(
        JitFrameDescriptor::new(
            JitFrameDescriptorId(5342),
            48,
            24,
            4,
            [
                safepoint(
                    4,
                    40,
                    SafepointKind::Allocation,
                    [ValueLocation::StackSlot(8), ValueLocation::FrameRegister(3)],
                ),
                safepoint(20, 56, SafepointKind::Bailout, []),
            ],
        )
        .expect("valid inner descriptor"),
    );

    let mut table = JitPcTable::default();
    table.install(0x1000, Arc::clone(&outer)).unwrap();
    table.install(0x2000, Arc::clone(&inner)).unwrap();

    let mut chain = JitFrameChain::default();
    chain
        .push(ActiveJitFrame {
            header: JitFrameHeader {
                frame_id: 10,
                descriptor_id: outer.id(),
                caller: FrameCaller::Interpreter { frame_depth: 2 },
            },
            safepoint_pc: 0x1008,
        })
        .unwrap();
    chain
        .push(ActiveJitFrame {
            header: JitFrameHeader {
                frame_id: 11,
                descriptor_id: inner.id(),
                caller: FrameCaller::Jit { frame_id: 10 },
            },
            safepoint_pc: 0x2004,
        })
        .unwrap();

    let resolved = chain.resolve_safepoints(&table).unwrap();
    assert_eq!(resolved.len(), 2);
    assert_eq!(resolved[0].safepoint.bytecode_offset, 12);
    assert_eq!(resolved[0].safepoint.kind, SafepointKind::Call);
    assert_eq!(
        resolved[0].safepoint.stack_map.live_values(),
        &[
            ValueLocation::MachineRegister(2),
            ValueLocation::StackSlot(-8),
            ValueLocation::FrameRegister(0),
        ]
    );
    assert_eq!(resolved[1].safepoint.bytecode_offset, 40);
    assert_eq!(resolved[1].safepoint.kind, SafepointKind::Allocation);
    assert_eq!(
        resolved[1].safepoint.stack_map.live_values(),
        &[ValueLocation::StackSlot(8), ValueLocation::FrameRegister(3),]
    );
    assert!(!resolved[0]
        .safepoint
        .stack_map
        .live_values()
        .contains(&ValueLocation::FrameRegister(1)));

    assert!(
        table.lookup(0x1009).is_none(),
        "lookup must require an exact PC"
    );
}

#[test]
fn malformed_metadata_and_caller_chains_fail_before_root_scanning() {
    assert!(matches!(
        JitFrameDescriptor::new(
            JitFrameDescriptorId(5343),
            16,
            8,
            1,
            [safepoint(
                4,
                0,
                SafepointKind::Bailout,
                [ValueLocation::FrameRegister(1)],
            )],
        ),
        Err(FrameMetadataError::FrameRegisterOutOfBounds { register: 1 })
    ));

    let mut chain = JitFrameChain::default();
    assert_eq!(
        chain.push(ActiveJitFrame {
            header: JitFrameHeader {
                frame_id: 20,
                descriptor_id: JitFrameDescriptorId(5344),
                caller: FrameCaller::Jit { frame_id: 19 },
            },
            safepoint_pc: 0x3000,
        }),
        Err(FrameMetadataError::BrokenCallerChain)
    );
}
