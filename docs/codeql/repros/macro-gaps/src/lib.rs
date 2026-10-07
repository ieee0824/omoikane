//! Reduced macro inputs from Omoikane Issue #1266, retaining ABI and Future checks.

use dynify::{Fn, from_fn};
use std::{any::Any, future::Future, mem::offset_of, rc::Rc};

#[repr(C)]
struct NativeFrame {
    registers: *mut i64,
    dirty: *mut u8,
}

static_assertions::const_assert!(offset_of!(NativeFrame, registers) == 0);
static_assertions::const_assert!(offset_of!(NativeFrame, dirty) == size_of::<*mut i64>());

#[repr(C)]
struct RuntimeCallFrame {
    trampoline: unsafe extern "C" fn(*mut RuntimeCallFrame),
    state: *mut std::ffi::c_void,
}

static_assertions::const_assert!(offset_of!(RuntimeCallFrame, trampoline) == 0);

trait Loader: Any {
    fn load<'a, 'b>(self: Rc<Self>, input: &'a &'b u32) -> impl Future<Output = u32>;
}

trait DynLoader: Any {
    fn load<'a, 'b, 'fut>(
        self: Rc<Self>,
        input: &'a &'b u32,
    ) -> Fn!(Rc<Self>, &'a &'b u32 => dyn 'fut + Future<Output = u32>)
    where
        'a: 'fut,
        'b: 'fut;
}

impl<T: Loader> DynLoader for T {
    fn load<'a, 'b, 'fut>(
        self: Rc<Self>,
        input: &'a &'b u32,
    ) -> Fn!(Rc<Self>, &'a &'b u32 => dyn 'fut + Future<Output = u32>)
    where
        'a: 'fut,
        'b: 'fut,
    {
        from_fn!(T::load, self, input)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_frame_layout_remains_checked() {
        assert_eq!(offset_of!(NativeFrame, registers), 0);
        assert_eq!(offset_of!(NativeFrame, dirty), size_of::<*mut i64>());
        assert_eq!(offset_of!(RuntimeCallFrame, trampoline), 0);
    }

    #[test]
    fn dyn_loader_retains_borrowed_future_contract() {
        struct Example;
        impl Loader for Example {
            async fn load<'a, 'b>(self: Rc<Self>, input: &'a &'b u32) -> u32 {
                **input
            }
        }
        let loader: Rc<dyn DynLoader> = Rc::new(Example);
        let value = 42;
        let reference = &value;
        let constructor = DynLoader::load(loader, &reference);
        let mut storage = Vec::<std::mem::MaybeUninit<u8>>::new();
        let future = dynify::Dynify::init(constructor, &mut storage);
        let mut future = std::pin::pin!(future);
        let mut context = std::task::Context::from_waker(std::task::Waker::noop());
        assert_eq!(
            future.as_mut().project().poll(&mut context),
            std::task::Poll::Ready(42)
        );
    }

    #[test]
    fn arm64_assertion_keeps_statement_cfg() {
        let dump = String::from("ldr x0, 0x8");
        assert!(dump.contains("ldr"));
        #[cfg(target_arch = "aarch64")]
        assert!(dump.contains("ldr x0, 0x8"));
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn arm64_assertion_with_function_cfg_control() {
        assert!(String::from("ldr x0, 0x8").contains("ldr x0, 0x8"));
    }
}
