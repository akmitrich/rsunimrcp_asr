use crate::speech_detector::{Detector8kHz, SpeechDetectorEvent};
use rsunimrcp_engine::Engine;
use rsunimrcp_sys::headers::RecogHeaders;
use std::{
    io::Write,
    sync::{mpsc, Arc},
};

#[derive(Debug)]
pub struct RecogBuffer {
    engine: Arc<Engine>,
    speech_detector: Detector8kHz,
    speech_detector_event: SpeechDetectorEvent,
    data_channel: (mpsc::Sender<String>, mpsc::Receiver<String>),
}

impl RecogBuffer {
    pub fn leaked(engine: Arc<Engine>) -> *mut Self {
        let instance = Self {
            engine,
            speech_detector: Detector8kHz::new(false, 200, 1000, 5000, 20000),
            speech_detector_event: SpeechDetectorEvent::None,
            data_channel: mpsc::channel(),
        };
        Box::into_raw(Box::new(instance))
    }

    pub unsafe fn destroy(this: *mut Self) {
        drop(Box::from_raw(this));
    }

    pub fn prepare(&mut self, headers: RecogHeaders) {
        self.speech_detector_event = SpeechDetectorEvent::None;
        let sensitivity = headers.sensitivity();
        self.speech_detector = Detector8kHz::new(
            headers.start_input_timers(),
            100,
            headers.silence_timeout(),
            headers.noinput_timeout(),
            headers.recognition_timeout(),
        );
        self.speech_detector.set_mode(sensitivity);
    }

    pub fn start_input_timers(&mut self) {
        self.speech_detector.timers_started = true;
    }

    pub fn start_input(&mut self) {
        self.speech_detector.input_started = true;
    }

    pub fn input_started(&self) -> bool {
        if self.speech_detector.timers_started {
            self.speech_detector.input_started
        } else {
            true
        }
    }
}

impl Write for RecogBuffer {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let next_event = self
            .speech_detector
            .process(buf, rsunimrcp_sys::uni::CODEC_FRAME_TIME_BASE as _);
        if self.speech_detector_event != SpeechDetectorEvent::Recognizing {
            self.speech_detector_event = next_event;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl RecogBuffer {
    pub fn detector_event(&self) -> SpeechDetectorEvent {
        self.speech_detector_event
    }

    pub fn duration_timeout(&self) -> usize {
        self.speech_detector.duration_timeout
    }

    fn decrease_noinput(&mut self, duration: usize) {
        if !self.speech_detector.timers_started {
            return;
        }
        if duration > self.speech_detector.noinput_timeout {
            self.speech_detector.noinput_timeout = 0
        } else {
            self.speech_detector.noinput_timeout -= duration
        }
    }

    pub fn restart_writing(&mut self) {
        self.speech_detector_event = SpeechDetectorEvent::None;
    }

    pub fn load_result(&self) -> Option<String> {
        let rx = &self.data_channel.1;
        match rx.try_recv() {
            Ok(result) => Some(result),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => {
                log::error!("Unable to load results from STT.");
                Some(String::new())
            }
        }
    }

    pub fn recognize(&mut self, duration: usize) {
        let data = std::mem::take(&mut self.speech_detector.speech);
        self.decrease_noinput(duration);
        self.speech_detector_event = SpeechDetectorEvent::Recognizing;
        log::info!("Send {} bytes to STT.", data.len());
        let tx = self.data_channel.0.clone();
        self.engine.async_handle().spawn(connect(data, tx));
    }
}

async fn connect(data: Vec<u8>, tx: mpsc::Sender<String>) {
    if data.is_empty() {
        tx.send(String::new()).unwrap();
        return;
    }
    let seconds = data.len() / 16000;
    let text = format!("Recognized {} seconds.", seconds);
    let _ = tx.send(text);
}
