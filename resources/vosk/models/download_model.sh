!#/bin/bash

# Download Vosk models
MODEL_URLS=(
    "https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip"
    "https://alphacephei.com/vosk/models/vosk-model-small-cn-0.22.zip"
)

for url in "${MODEL_URLS[@]}"; do
    wget "$url"
done