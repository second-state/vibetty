use reqwest::multipart::Part;

pub mod bailian;

#[derive(Debug, serde::Deserialize)]
struct AsrResult {
    #[serde(default)]
    text: String,
}

impl AsrResult {
    fn parse_text(self) -> Vec<String> {
        let mut texts = vec![];
        for line in self.text.lines() {
            if let Some((_, t)) = line.split_once("] ") {
                texts.push(t.to_string());
            } else {
                texts.push(line.to_string());
            }
        }
        texts
    }
}

/// wav_audio: 16bit,16k,single-channel.
pub async fn whisper(
    client: &reqwest::Client,
    asr_url: &str,
    api_key: &str,
    model: &str,
    lang: &str,
    prompt: &str,
    wav_audio: Vec<u8>,
) -> anyhow::Result<Vec<String>> {
    let mut form =
        reqwest::multipart::Form::new().part("file", Part::bytes(wav_audio).file_name("audio.wav"));

    if !lang.is_empty() {
        form = form.text("language", lang.to_string());
    }

    if !model.is_empty() {
        form = form.text("model", model.to_string());
    }

    if !prompt.is_empty() {
        form = form.text("prompt", prompt.to_string());
    }

    let builder = client.post(asr_url).multipart(form);

    let res = if !api_key.is_empty() {
        builder
            .bearer_auth(api_key)
            .header(reqwest::header::USER_AGENT, "curl/7.81.0")
    } else {
        builder
    }
    .send()
    .await?;

    let r: serde_json::Value = res.json().await?;
    log::debug!("ASR response: {:#?}", r);

    let asr_result: AsrResult = serde_json::from_value(r)
        .map_err(|e| anyhow::anyhow!("Failed to parse ASR result: {}", e))?;
    Ok(asr_result.parse_text())
}

#[tokio::test]
#[ignore]
async fn test_groq_asr() {
    env_logger::init();
    let groq_api_key = std::env::var("GROQ_API_KEY").unwrap_or_default();
    let test_wav_path = std::env::var("TEST_WAV_PATH").unwrap();
    let asr_url = "https://api.groq.com/openai/v1/audio/transcriptions";
    let lang = "zh";
    let wav_audio = std::fs::read(test_wav_path).unwrap();
    let client = reqwest::Client::new();

    let text = whisper(
        &client,
        asr_url,
        &groq_api_key,
        "whisper-large-v3",
        lang,
        "",
        wav_audio,
    )
    .await
    .unwrap();
    println!("ASR result: {:?}", text);
}
