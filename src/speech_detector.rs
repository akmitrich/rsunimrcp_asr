const MAX_SENSITIVITY: usize = 1200;

#[derive(Debug, PartialEq, Clone, Copy)]
pub enum SpeechDetectorEvent {
    None,
    Activity,
    Inactivity { duration: usize },
    Noinput,
    DurationTimeout,
    Recognizing,
}

#[derive(Debug)]
enum SpeechDetectorState {
    Inactivity,
    ActivityTransition,
    Activity,
    InactivityTransition,
    Exhausted,
}

#[allow(unused)]
#[derive(Debug)]
pub struct Detector8kHz {
    pub speech: Vec<u8>,
    speech_timeout: usize,
    silence_timeout: usize,
    pub timers_started: bool,
    pub input_started: bool,
    pub noinput_timeout: usize,
    pub duration_timeout: usize,
    inactivity_duration: usize,
    activity_duration: usize,
    pub total_duration: usize,
    in_sensitivity: usize,
    out_sensitivity: usize,
    state: SpeechDetectorState,
}

impl Detector8kHz {
    pub fn new(
        timers_started: bool,
        speech_timeout: usize,
        silence_timeout: usize,
        noinput_timeout: usize,
        duration_timeout: usize,
    ) -> Self {
        Self {
            speech: vec![],
            speech_timeout,
            silence_timeout,
            timers_started,
            input_started: false,
            noinput_timeout,
            duration_timeout,
            inactivity_duration: 0,
            activity_duration: 0,
            total_duration: 0,
            in_sensitivity: 32,
            out_sensitivity: 512,
            state: SpeechDetectorState::Inactivity,
        }
    }

    pub fn set_mode(&mut self, mode: f64) {
        let sensitivity = if (0.0..=1.0).contains(&mode) {
            mode
        } else {
            1.0
        };
        self.in_sensitivity = (sensitivity * 100.0) as _;
        let out_sensitivity = self.in_sensitivity << 4;
        self.out_sensitivity = if out_sensitivity > MAX_SENSITIVITY {
            MAX_SENSITIVITY
        } else {
            out_sensitivity
        };
    }

    pub fn process(&mut self, frame: &[u8], duration: usize) -> SpeechDetectorEvent {
        let mut result = SpeechDetectorEvent::None;
        match self.state {
            SpeechDetectorState::Inactivity => {
                self.change_state(SpeechDetectorState::ActivityTransition);
            }
            SpeechDetectorState::ActivityTransition => {
                self.activity_duration = duration;
                self.change_state(SpeechDetectorState::Activity);
                result = SpeechDetectorEvent::Activity;
            }
            SpeechDetectorState::Activity => {
                self.activity_duration += duration;
                self.speech.extend_from_slice(frame);
                if self.speech.len() > 16000 {
                    self.change_state(SpeechDetectorState::InactivityTransition);
                }
            }
            SpeechDetectorState::InactivityTransition => {
                self.speech.extend_from_slice(frame);
                self.change_state(SpeechDetectorState::Inactivity);
                result = SpeechDetectorEvent::Inactivity {
                    duration: self.activity_duration,
                };
            }
            SpeechDetectorState::Exhausted => return SpeechDetectorEvent::Noinput,
        }
        self.total_duration += duration;
        if self.total_duration >= self.duration_timeout {
            result = SpeechDetectorEvent::DurationTimeout;
            self.change_state(SpeechDetectorState::Exhausted);
        }
        result
    }
}

impl Detector8kHz {
    fn change_state(&mut self, state: SpeechDetectorState) {
        self.state = state;
    }
}
