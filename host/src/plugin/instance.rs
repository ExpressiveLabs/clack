use crate::extensions::wrapper::descriptor::RawHostDescriptor;
use crate::extensions::wrapper::HostWrapper;
use crate::prelude::*;
use clap_sys::plugin::clap_plugin;
use std::ffi::CStr;
use std::pin::Pin;
use std::sync::Arc;

pub struct PluginInstanceInner<H: Host> {
    host_wrapper: Pin<Box<HostWrapper<H>>>,
    host_descriptor: Pin<Box<RawHostDescriptor>>,
    plugin_ptr: *mut clap_plugin,
    _plugin_bundle: PluginBundle, // SAFETY: Keep the DLL/.SO alive while plugin is instantiated
}

impl<H: Host> PluginInstanceInner<H> {
    pub(crate) fn instantiate<FH, FS>(
        shared: FS,
        main_thread: FH,
        entry: &PluginBundle,
        plugin_id: &CStr,
        host_info: HostInfo,
    ) -> Result<Arc<Self>, HostError>
    where
        FS: for<'s> FnOnce(&'s ()) -> <H as Host>::Shared<'s>,
        FH: for<'s> FnOnce(&'s <H as Host>::Shared<'s>) -> <H as Host>::MainThread<'s>,
    {
        let host_wrapper = HostWrapper::new(shared, main_thread);
        let host_descriptor = Box::pin(RawHostDescriptor::new::<H>(host_info));

        let mut instance = Arc::new(Self {
            host_wrapper,
            host_descriptor,
            plugin_ptr: core::ptr::null_mut(),
            _plugin_bundle: entry.clone(),
        });

        {
            let instance = Arc::get_mut(&mut instance).unwrap();
            instance.host_descriptor.set_wrapper(&instance.host_wrapper);

            let raw_descriptor = instance.host_descriptor.raw();

            // SAFETY: the host pointer comes from a valid allocation that is pinned for the
            // lifetime of the instance
            let plugin_instance_ptr = unsafe {
                entry
                    .get_plugin_factory()
                    .ok_or(HostError::MissingPluginFactory)?
                    .create_plugin(plugin_id, raw_descriptor)?
                    .as_ptr()
            };

            if plugin_instance_ptr.is_null() {
                return Err(HostError::InstantiationFailed);
            }

            // SAFETY: we just checked the pointer is non-null
            unsafe {
                instance
                    .host_wrapper
                    .as_mut()
                    .instantiated(plugin_instance_ptr)
            };
            instance.plugin_ptr = plugin_instance_ptr;
        }

        Ok(instance)
    }

    #[inline]
    pub fn wrapper(&self) -> &HostWrapper<H> {
        &self.host_wrapper
    }

    #[inline]
    pub fn raw_instance(&self) -> &clap_plugin {
        // SAFETY: this type ensures the instance pointer is valid
        unsafe { &*self.plugin_ptr }
    }

    pub fn activate<FA>(
        &mut self,
        audio_processor: FA,
        configuration: PluginAudioConfiguration,
    ) -> Result<(), HostError>
    where
        FA: for<'a> FnOnce(
            PluginAudioProcessorHandle<'a>,
            &'a <H as Host>::Shared<'a>,
            &mut <H as Host>::MainThread<'a>,
        ) -> <H as Host>::AudioProcessor<'a>,
    {
        let activate = self
            .raw_instance()
            .activate
            .ok_or(HostError::NullActivateFunction)?;

        // FIXME: reentrancy if activate() calls audio_processor methods
        self.host_wrapper
            .as_mut()
            .setup_audio_processor(audio_processor, self.plugin_ptr)?;

        // SAFETY: this type ensures the function pointer is valid
        let success = unsafe {
            activate(
                self.plugin_ptr,
                configuration.sample_rate,
                *configuration.frames_count_range.start(),
                *configuration.frames_count_range.end(),
            )
        };

        if !success {
            let _ = self.host_wrapper.as_mut().deactivate(|_, _| ());
            return Err(HostError::ActivationFailed);
        }

        Ok(())
    }

    #[inline]
    pub fn deactivate_with<T>(
        &mut self,
        drop: impl for<'s> FnOnce(
            <H as Host>::AudioProcessor<'s>,
            &mut <H as Host>::MainThread<'s>,
        ) -> T,
    ) -> Result<T, HostError> {
        if !self.wrapper().is_active() {
            return Err(HostError::DeactivatedPlugin);
        }

        if let Some(deactivate) = self.raw_instance().deactivate {
            // SAFETY: this type ensures the function pointer is valid.
            // We just checked the instance is in an active state.
            unsafe { deactivate(self.plugin_ptr) };
        }

        self.host_wrapper.as_mut().deactivate(drop)
    }

    /// # Safety
    /// User must ensure the instance is not in a processing state.
    #[inline]
    pub unsafe fn start_processing(&self) -> Result<(), HostError> {
        if let Some(start_processing) = (*self.plugin_ptr).start_processing {
            if start_processing(self.plugin_ptr) {
                return Ok(());
            }

            Err(HostError::StartProcessingFailed)
        } else {
            Ok(())
        }
    }

    /// # Safety
    /// User must ensure the instance is in a processing state.
    #[inline]
    pub unsafe fn stop_processing(&self) {
        if let Some(stop_processing) = (*self.plugin_ptr).stop_processing {
            stop_processing(self.plugin_ptr)
        }
    }

    /// # Safety
    /// User must ensure this is only called on the main thread.
    #[inline]
    pub unsafe fn on_main_thread(&self) {
        if let Some(on_main_thread) = (*self.plugin_ptr).on_main_thread {
            on_main_thread(self.plugin_ptr)
        }
    }
}

impl<H: Host> Drop for PluginInstanceInner<H> {
    #[inline]
    fn drop(&mut self) {
        // Happens only if instantiate didn't complete
        if self.plugin_ptr.is_null() {
            return;
        }

        // Check if instance hasn't been properly deactivated
        if self.host_wrapper.is_active() {
            let _ = self.deactivate_with(|_, _| ());
        }

        // SAFETY: we are in the drop impl, so this can only be called once and without any
        // other concurrent calls.
        // This type also ensures the function pointer type is valid.
        unsafe {
            if let Some(destroy) = (*self.plugin_ptr).destroy {
                destroy(self.plugin_ptr)
            }
        }
    }
}

// SAFETY: The only non-thread-safe methods on this type are unsafe
unsafe impl<H: Host> Send for PluginInstanceInner<H> {}
// SAFETY: The only non-thread-safe methods on this type are unsafe
unsafe impl<H: Host> Sync for PluginInstanceInner<H> {}
