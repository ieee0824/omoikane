//! Owned provider boundary for Web Speech synthesis and recognition.

use super::*;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};

/// One speech-synthesis voice exposed by the host.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechVoice {
    /// Stable provider-specific voice identifier.
    pub voice_uri: String,
    /// Human-readable voice name.
    pub name: String,
    /// BCP 47 language tag.
    pub lang: String,
    /// Whether synthesis runs without a remote service.
    pub local_service: bool,
    /// Whether this is the provider's default voice.
    pub default: bool,
}

/// An owned utterance sent to the synthesis backend.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechSynthesisRequest {
    /// Runtime-local utterance identifier.
    pub id: u64,
    /// Text snapshot to speak.
    pub text: String,
    /// Requested BCP 47 language tag.
    pub lang: String,
    /// Selected provider voice, when any.
    pub voice_uri: Option<String>,
    /// Volume in the inclusive range 0 through 1.
    pub volume: f64,
    /// Speaking rate in the inclusive range 0.1 through 10.
    pub rate: f64,
    /// Pitch in the inclusive range 0 through 2.
    pub pitch: f64,
}

/// An owned phrase hint for speech recognition.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechRecognitionPhrase {
    /// Phrase supplied as contextual recognition input.
    pub phrase: String,
    /// Positive relative phrase weight.
    pub boost: f64,
}

/// A complete recognition session configuration.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpeechRecognitionRequest {
    /// Runtime-local recognition session identifier.
    pub id: u64,
    /// Requested BCP 47 language tag.
    pub lang: String,
    /// Whether the session may return multiple final results.
    pub continuous: bool,
    /// Whether interim results should be delivered.
    pub interim_results: bool,
    /// Maximum alternatives requested per result.
    pub max_alternatives: u32,
    /// Whether recognition must remain on device.
    pub process_locally: bool,
    /// Owned contextual phrase hints.
    pub phrases: Vec<SpeechRecognitionPhrase>,
}

/// Commands whose lifetime and side effects belong to the host adapter.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum WebSpeechCommand {
    /// Starts synthesis for one owned utterance.
    Speak(SpeechSynthesisRequest),
    /// Cancels the current and queued synthesis work.
    CancelSynthesis,
    /// Pauses synthesis output.
    PauseSynthesis,
    /// Resumes synthesis output.
    ResumeSynthesis,
    /// Starts a recognition session.
    StartRecognition(SpeechRecognitionRequest),
    /// Requests a graceful end to a recognition session.
    StopRecognition { id: u64 },
    /// Aborts a recognition session.
    AbortRecognition { id: u64 },
    /// Releases every provider resource owned by the runtime.
    Teardown,
}

/// A host-side recognition failure converted to `SpeechRecognitionErrorEvent`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpeechRecognitionFailure {
    /// The document or user denied recognition.
    NotAllowed,
    /// The configured recognition service is unavailable to the document.
    ServiceNotAllowed,
    /// No usable audio capture source exists.
    AudioCapture,
    /// The recognition service encountered a network failure.
    Network,
    /// The requested language is unsupported.
    LanguageNotSupported,
}

impl SpeechRecognitionFailure {
    fn as_str(self) -> &'static str {
        match self {
            Self::NotAllowed => "not-allowed",
            Self::ServiceNotAllowed => "service-not-allowed",
            Self::AudioCapture => "audio-capture",
            Self::Network => "network",
            Self::LanguageNotSupported => "language-not-supported",
        }
    }
}

/// Embedder contract for native speech services or deterministic test doubles.
pub trait WebSpeechProvider: Send + Sync {
    /// Returns one owned voice inventory generation.
    fn voices(&self) -> Vec<SpeechVoice>;
    /// Applies one already validated command.
    fn command(&self, command: WebSpeechCommand) -> Result<(), SpeechRecognitionFailure>;
}

/// Headless provider that records commands and rejects microphone recognition.
#[derive(Debug)]
pub struct DeterministicWebSpeechProvider {
    voices: Vec<SpeechVoice>,
    commands: Mutex<Vec<WebSpeechCommand>>,
    recognition_failure: Option<SpeechRecognitionFailure>,
}

impl Default for DeterministicWebSpeechProvider {
    fn default() -> Self {
        Self {
            voices: Vec::new(),
            commands: Mutex::new(Vec::new()),
            recognition_failure: Some(SpeechRecognitionFailure::NotAllowed),
        }
    }
}

impl DeterministicWebSpeechProvider {
    /// Creates a provider with an explicit immutable voice inventory.
    pub fn new(voices: Vec<SpeechVoice>) -> Self {
        Self {
            voices,
            commands: Mutex::new(Vec::new()),
            recognition_failure: Some(SpeechRecognitionFailure::NotAllowed),
        }
    }

    /// Creates a provider that accepts recognition and awaits explicit host events.
    pub fn accepting(voices: Vec<SpeechVoice>) -> Self {
        Self {
            voices,
            commands: Mutex::new(Vec::new()),
            recognition_failure: None,
        }
    }

    /// Returns commands in host-observation order.
    pub fn commands(&self) -> Vec<WebSpeechCommand> {
        self.commands.lock().unwrap().clone()
    }
}

impl WebSpeechProvider for DeterministicWebSpeechProvider {
    fn voices(&self) -> Vec<SpeechVoice> {
        self.voices.clone()
    }

    fn command(&self, command: WebSpeechCommand) -> Result<(), SpeechRecognitionFailure> {
        let rejects_recognition = matches!(command, WebSpeechCommand::StartRecognition(_));
        self.commands.lock().unwrap().push(command);
        if rejects_recognition {
            self.recognition_failure.map_or(Ok(()), Err)
        } else {
            Ok(())
        }
    }
}

pub(super) struct State {
    provider: Arc<dyn WebSpeechProvider>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            provider: Arc::new(DeterministicWebSpeechProvider::default()),
        }
    }
}

impl Drop for State {
    fn drop(&mut self) {
        let _ = self.provider.command(WebSpeechCommand::Teardown);
    }
}

fn operation_native(_: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = string_argument(args.first(), "", context)?;
    let json = string_argument(args.get(1), "", context)?;
    with_host_state(|host| {
        let provider = host.borrow().web_speech.provider.clone();
        match operation.as_str() {
            "voices" => Ok(JsValue::from(JsString::from(
                serde_json::to_string(&provider.voices()).unwrap(),
            ))),
            "command" => {
                let command: WebSpeechCommand = serde_json::from_str(&json)
                    .map_err(|_| JsNativeError::typ().with_message("Invalid Web Speech command"))?;
                match provider.command(command) {
                    Ok(()) => Ok(JsValue::null()),
                    Err(error) => Ok(JsValue::from(JsString::from(error.as_str()))),
                }
            }
            _ => Err(JsNativeError::typ()
                .with_message("Unknown Web Speech operation")
                .into()),
        }
    })
}

pub(super) fn register(context: &mut Context, bindings: &mut BootstrapBindings) -> JsResult<()> {
    register_private_builtin_callable(
        context,
        bindings,
        js_string!("__omoikane_web_speech"),
        2,
        NativeFunction::from_copy_closure(operation_native),
    )
}

impl JsRuntime {
    /// Replaces the backend used by subsequent speech operations.
    pub fn set_web_speech_provider(&mut self, provider: Arc<dyn WebSpeechProvider>) {
        self.host_state.borrow_mut().web_speech.provider = provider;
    }

    /// Delivers a synthesis lifecycle event for the current utterance.
    pub fn dispatch_speech_synthesis_event(
        &mut self,
        event: &str,
        details: serde_json::Value,
    ) -> JsResult<()> {
        let source = format!(
            "__omoikane_dispatch_speech_synthesis_event({},{})",
            serde_json::to_string(event).unwrap(),
            serde_json::to_string(&details).unwrap()
        );
        self.eval(&source)?;
        Ok(())
    }

    /// Delivers one recognition lifecycle/result/error event by session id.
    pub fn dispatch_speech_recognition_event(
        &mut self,
        id: u64,
        event: &str,
        details: serde_json::Value,
    ) -> JsResult<()> {
        let source = format!(
            "__omoikane_dispatch_speech_recognition_event({id},{},{})",
            serde_json::to_string(event).unwrap(),
            serde_json::to_string(&details).unwrap()
        );
        self.eval(&source)?;
        Ok(())
    }
}
