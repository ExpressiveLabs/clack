use self::audio_buffers::InputAudioBuffers;
use crate::host::HostError;
use crate::host::HostHandlers;
use crate::plugin::{PluginAudioProcessorHandle, PluginSharedHandle};
use crate::prelude::{OutputAudioBuffers, PluginInstance};
use crate::process::PluginAudioProcessor::*;
use clack_common::events::event_types::TransportEvent;
use clack_common::events::io::{InputEvents, OutputEvents};
use clap_sys::process::clap_process;
use std::cell::UnsafeCell;
use std::error::Error;
use std::fmt::{Debug, Display, Formatter};
use std::marker::PhantomData;
use std::sync::Arc;

use crate::plugin::instance::PluginInstanceInner;
pub use clack_common::process::*;

pub mod audio_buffers;

pub enum PluginAudioProcessor<H: HostHandlers> {
    Started(StartedPluginAudioProcessor<H>),
    Stopped(StoppedPluginAudioProcessor<H>),
    Poisoned,
}

impl<'a, H: 'a + HostHandlers> PluginAudioProcessor<H> {
    #[inline]
    pub fn as_started(&self) -> Result<&StartedPluginAudioProcessor<H>, HostError> {
        match self {
            Started(s) => Ok(s),
            Stopped(_) => Err(HostError::ProcessingStopped),
            Poisoned => unreachable!(),
        }
    }

    #[inline]
    pub fn as_started_mut(&mut self) -> Result<&mut StartedPluginAudioProcessor<H>, HostError> {
        match self {
            Started(s) => Ok(s),
            Stopped(_) => Err(HostError::ProcessingStopped),
            Poisoned => unreachable!(),
        }
    }

    #[inline]
    pub fn as_stopped(&self) -> Result<&StoppedPluginAudioProcessor<H>, HostError> {
        match self {
            Stopped(s) => Ok(s),
            Started(_) => Err(HostError::ProcessingStarted),
            Poisoned => unreachable!(),
        }
    }

    #[inline]
    pub fn as_stopped_mut(&mut self) -> Result<&mut StoppedPluginAudioProcessor<H>, HostError> {
        match self {
            Stopped(s) => Ok(s),
            Started(_) => Err(HostError::ProcessingStarted),
            Poisoned => unreachable!(),
        }
    }

    #[inline]
    pub fn shared_handler(&self) -> &<H as HostHandlers>::Shared<'_> {
        match self {
            Poisoned => panic!("Plugin audio processor was poisoned"),
            Started(s) => s.shared(),
            Stopped(s) => s.shared(),
        }
    }

    #[inline]
    pub fn handler(&self) -> &<H as HostHandlers>::AudioProcessor<'_> {
        match self {
            Poisoned => panic!("Plugin audio processor was poisoned"),
            Started(s) => s.handler(),
            Stopped(s) => s.handler(),
        }
    }

    #[inline]
    pub fn handler_mut(&mut self) -> &mut <H as HostHandlers>::AudioProcessor<'_> {
        match self {
            Poisoned => panic!("Plugin audio processor was poisoned"),
            Started(s) => s.handler_mut(),
            Stopped(s) => s.handler_mut(),
        }
    }

    pub fn is_started(&self) -> bool {
        match self {
            Stopped(_) | Poisoned => false,
            Started(_) => true,
        }
    }

    #[inline]
    pub fn plugin_handle(&mut self) -> PluginAudioProcessorHandle {
        match self {
            Poisoned => panic!("Plugin audio processor was poisoned"),
            Started(s) => s.plugin_handle(),
            Stopped(s) => s.plugin_handle(),
        }
    }

    #[inline]
    pub fn shared_plugin_handle(&self) -> PluginSharedHandle {
        match self {
            Poisoned => panic!("Plugin audio processor was poisoned"),
            Started(s) => s.shared_plugin_handle(),
            Stopped(s) => s.shared_plugin_handle(),
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        match self {
            Started(s) => s.reset(),
            Stopped(s) => s.reset(),
            Poisoned => unreachable!(),
        }
    }

    pub fn ensure_processing_started(
        &mut self,
    ) -> Result<&mut StartedPluginAudioProcessor<H>, HostError> {
        match self {
            Started(s) => Ok(s),
            _ => self.start_processing(),
        }
    }

    pub fn start_processing(&mut self) -> Result<&mut StartedPluginAudioProcessor<H>, HostError> {
        let inner = core::mem::replace(self, Poisoned);

        match inner {
            Poisoned => unreachable!("Audio processor handle somehow panicked and got poisoned."),
            Started(s) => {
                *self = Started(s);
                Err(HostError::ProcessingStarted)
            }
            Stopped(s) => match s.start_processing() {
                Ok(s) => {
                    *self = Started(s);
                    Ok(match self {
                        Started(s) => s,
                        _ => unreachable!(),
                    })
                }
                Err(e) => {
                    *self = Stopped(e.processor);
                    Err(HostError::StartProcessingFailed)
                }
            },
        }
    }

    pub fn ensure_processing_stopped(&mut self) -> &mut StoppedPluginAudioProcessor<H> {
        let inner = core::mem::replace(self, Poisoned);

        match inner {
            Poisoned => unreachable!(),
            Stopped(s) => {
                *self = Stopped(s);
            }
            Started(s) => {
                *self = Stopped(s.stop_processing());
            }
        }

        match self {
            Stopped(s) => s,
            _ => unreachable!(),
        }
    }

    pub fn stop_processing(&mut self) -> Result<&mut StoppedPluginAudioProcessor<H>, HostError> {
        let inner = core::mem::replace(self, Poisoned);

        match inner {
            Poisoned => unreachable!(),
            Stopped(s) => {
                *self = Stopped(s);
                Err(HostError::ProcessingStopped)
            }
            Started(s) => {
                *self = Stopped(s.stop_processing());
                Ok(match self {
                    Stopped(s) => s,
                    _ => unreachable!(),
                })
            }
        }
    }

    pub fn into_started(self) -> Result<StartedPluginAudioProcessor<H>, ProcessingStartError<H>> {
        match self {
            Started(s) => Ok(s),
            Stopped(s) => s.start_processing(),
            Poisoned => unreachable!(),
        }
    }

    pub fn into_stopped(self) -> StoppedPluginAudioProcessor<H> {
        match self {
            Started(s) => s.stop_processing(),
            Stopped(s) => s,
            Poisoned => unreachable!(),
        }
    }

    #[inline]
    pub fn matches(&self, instance: &PluginInstance<H>) -> bool {
        match &self {
            Started(s) => s.matches(instance),
            Stopped(s) => s.matches(instance),
            Poisoned => unreachable!(),
        }
    }
}

impl<'a, H: 'a + HostHandlers> From<StartedPluginAudioProcessor<H>> for PluginAudioProcessor<H> {
    #[inline]
    fn from(p: StartedPluginAudioProcessor<H>) -> Self {
        Started(p)
    }
}

impl<'a, H: 'a + HostHandlers> From<StoppedPluginAudioProcessor<H>> for PluginAudioProcessor<H> {
    #[inline]
    fn from(p: StoppedPluginAudioProcessor<H>) -> Self {
        Stopped(p)
    }
}

pub struct StartedPluginAudioProcessor<H: HostHandlers> {
    inner: Option<Arc<PluginInstanceInner<H>>>,
    _no_sync: PhantomData<UnsafeCell<()>>,
}

impl<H: HostHandlers> StartedPluginAudioProcessor<H> {
    pub fn process(
        &mut self,
        audio_inputs: &InputAudioBuffers,
        audio_outputs: &mut OutputAudioBuffers,
        events_input: &InputEvents,
        events_output: &mut OutputEvents,
        steady_time: Option<u64>,
        transport: Option<&TransportEvent>,
    ) -> Result<ProcessStatus, HostError> {
        // TODO: add test for this
        let frames_count = match (audio_inputs.frames_count(), audio_outputs.frames_count()) {
            (Some(a), Some(b)) => a.min(b),
            (Some(a), None) | (None, Some(a)) => a,
            (None, None) => 0,
        };

        let process = clap_process {
            steady_time: match steady_time {
                None => -1,
                Some(steady_time) => steady_time.min(i64::MAX as u64) as i64,
            },
            frames_count,
            transport: transport
                .map(|e| e.as_raw_ref() as *const _)
                .unwrap_or(core::ptr::null()),
            audio_inputs_count: audio_inputs.as_raw_buffers().len() as u32,
            audio_outputs_count: audio_outputs.as_raw_buffers().len() as u32,
            audio_inputs: audio_inputs.as_raw_buffers().as_ptr(),
            audio_outputs: audio_outputs.as_raw_buffers().as_mut_ptr(),
            in_events: events_input.as_raw(),
            out_events: events_output.as_raw_mut() as *mut _,
        };

        let instance = self.inner.as_ref().unwrap().raw_instance();

        // SAFETY: this type ensures the function pointer is valid
        let status = ProcessStatus::from_raw(unsafe {
            instance.process.ok_or(HostError::NullProcessFunction)?(instance, &process)
        })
        .ok_or(())
        .and_then(|r| r)
        .map_err(|_| HostError::ProcessingFailed)?;

        Ok(status)
    }

    #[inline]
    pub fn reset(&mut self) {
        // SAFETY: This type ensures this can only be called in the main thread.
        unsafe { self.inner.as_mut().unwrap().reset() }
    }

    #[inline]
    pub fn stop_processing(mut self) -> StoppedPluginAudioProcessor<H> {
        let inner = self.inner.take().unwrap();
        // SAFETY: this is called on the audio thread
        unsafe { inner.stop_processing() };

        StoppedPluginAudioProcessor {
            inner,
            _no_sync: PhantomData,
        }
    }

    #[inline]
    pub fn shared(&self) -> &<H as HostHandlers>::Shared<'_> {
        self.inner.as_ref().unwrap().wrapper().shared()
    }

    #[inline]
    pub fn handler(&self) -> &<H as HostHandlers>::AudioProcessor<'_> {
        // SAFETY: we take &self, the only reference to the wrapper on the audio thread, therefore
        // we can guarantee there are no mutable references anywhere
        // PANIC: This struct exists, therefore we are guaranteed the plugin is active
        unsafe {
            self.inner
                .as_ref()
                .unwrap()
                .wrapper()
                .audio_processor()
                .unwrap()
                .as_ref()
        }
    }

    #[inline]
    pub fn handler_mut(&mut self) -> &mut <H as HostHandlers>::AudioProcessor<'_> {
        // SAFETY: we take &mut self, the only reference to the wrapper on the audio thread,
        // therefore we can guarantee there are other references anywhere
        // PANIC: This struct exists, therefore we are guaranteed the plugin is active
        unsafe {
            self.inner
                .as_ref()
                .unwrap()
                .wrapper()
                .audio_processor()
                .unwrap()
                .as_mut()
        }
    }

    #[inline]
    pub fn shared_plugin_handle(&self) -> PluginSharedHandle {
        self.inner.as_ref().unwrap().plugin_shared()
    }

    #[inline]
    pub fn plugin_handle(&mut self) -> PluginAudioProcessorHandle {
        PluginAudioProcessorHandle::new(self.inner.as_ref().unwrap().raw_instance().into())
    }

    #[inline]
    pub fn matches(&self, instance: &PluginInstance<H>) -> bool {
        Arc::ptr_eq(self.inner.as_ref().unwrap(), instance.inner.get())
    }
}

impl<H: HostHandlers> Drop for StartedPluginAudioProcessor<H> {
    fn drop(&mut self) {
        if let Some(inner) = self.inner.take() {
            // SAFETY: this is called on the audio thread
            unsafe { inner.stop_processing() };
        }
    }
}

pub struct StoppedPluginAudioProcessor<H: HostHandlers> {
    pub(crate) inner: Arc<PluginInstanceInner<H>>,
    _no_sync: PhantomData<UnsafeCell<()>>,
}

impl<'a, H: 'a + HostHandlers> StoppedPluginAudioProcessor<H> {
    #[inline]
    pub(crate) fn new(inner: Arc<PluginInstanceInner<H>>) -> Self {
        Self {
            inner,
            _no_sync: PhantomData,
        }
    }

    #[inline]
    pub fn reset(&mut self) {
        // SAFETY: This type ensures this can only be called in the main thread.
        unsafe { self.inner.reset() }
    }

    #[inline]
    pub fn start_processing(
        self,
    ) -> Result<StartedPluginAudioProcessor<H>, ProcessingStartError<H>> {
        // SAFETY: this is called on the audio thread
        match unsafe { self.inner.start_processing() } {
            Ok(()) => Ok(StartedPluginAudioProcessor {
                inner: Some(self.inner),
                _no_sync: PhantomData,
            }),
            Err(_) => Err(ProcessingStartError { processor: self }),
        }
    }

    #[inline]
    pub fn matches(&self, instance: &PluginInstance<H>) -> bool {
        Arc::ptr_eq(&self.inner, instance.inner.get())
    }

    #[inline]
    pub fn shared(&self) -> &<H as HostHandlers>::Shared<'_> {
        self.inner.wrapper().shared()
    }

    #[inline]
    pub fn handler(&self) -> &<H as HostHandlers>::AudioProcessor<'_> {
        // SAFETY: we take &self, the only reference to the wrapper on the audio thread, therefore
        // we can guarantee there are no mutable references anywhere
        // PANIC: This struct exists, therefore we are guaranteed the plugin is active
        unsafe { self.inner.wrapper().audio_processor().unwrap().as_ref() }
    }

    #[inline]
    pub fn handler_mut(&mut self) -> &mut <H as HostHandlers>::AudioProcessor<'_> {
        // SAFETY: we take &mut self, the only reference to the wrapper on the audio thread,
        // therefore we can guarantee there are other references anywhere
        // PANIC: This struct exists, therefore we are guaranteed the plugin is active
        unsafe { self.inner.wrapper().audio_processor().unwrap().as_mut() }
    }

    #[inline]
    pub fn shared_plugin_handle(&self) -> PluginSharedHandle {
        self.inner.plugin_shared()
    }

    #[inline]
    pub fn plugin_handle(&mut self) -> PluginAudioProcessorHandle {
        PluginAudioProcessorHandle::new(self.inner.raw_instance().into())
    }
}

pub struct ProcessingStartError<H: HostHandlers> {
    processor: StoppedPluginAudioProcessor<H>,
}

impl<H: HostHandlers> ProcessingStartError<H> {
    #[inline]
    pub fn into_stopped_processor(self) -> StoppedPluginAudioProcessor<H> {
        self.processor
    }
}

impl<H: HostHandlers> Debug for ProcessingStartError<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("Failed to start plugin processing")
    }
}

impl<H: HostHandlers> Display for ProcessingStartError<H> {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        f.write_str("Failed to start plugin processing")
    }
}

impl<H: HostHandlers> Error for ProcessingStartError<H> {}

#[cfg(test)]
mod test {
    extern crate static_assertions as sa;
    use super::*;

    sa::assert_not_impl_any!(StartedPluginAudioProcessor<()>: Sync);
    sa::assert_not_impl_any!(StoppedPluginAudioProcessor<()>: Sync);
}
