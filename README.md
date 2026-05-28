<div align="center" style="border-bottom: none">
    <h1>
        <img src="docs/Meetily-6.png" style="border-radius: 10px;" />
        <br>
        Privacy-First AI Meeting Assistant
    </h1>
    <a href="https://trendshift.io/repositories/21958" target="_blank"><img src="https://trendshift.io/api/badge/repositories/21958" alt="Zackriya-Solutions%2Fmeetily | Trendshift" style="width: 250px; height: 55px;" width="250" height="55"/></a>
    <br>
    <br>
    <a href="https://github.com/Zackriya-Solutions/meeting-minutes/releases/"><img src="https://img.shields.io/badge/Pre_Release-Link-brightgreen" alt="Pre-Release"></a>
    <a href="https://github.com/Zackriya-Solutions/meeting-minutes/releases"><img alt="GitHub Repo stars" src="https://img.shields.io/github/stars/zackriya-solutions/meeting-minutes?style=flat">
</a>
 <a href="https://github.com/Zackriya-Solutions/meeting-minutes/releases"> <img alt="GitHub Downloads (all assets, all releases)" src="https://img.shields.io/github/downloads/zackriya-solutions/meeting-minutes/total?style=plastic"> </a>
    <a href="https://github.com/Zackriya-Solutions/meeting-minutes/releases"><img src="https://img.shields.io/badge/License-MIT-blue" alt="License"></a>
    <a href="https://github.com/Zackriya-Solutions/meeting-minutes/releases"><img src="https://img.shields.io/badge/Supported_OS-macOS,_Windows-white" alt="Supported OS"></a>
    <a href="https://github.com/Zackriya-Solutions/meeting-minutes/releases"><img alt="GitHub Tag" src="https://img.shields.io/github/v/tag/zackriya-solutions/meeting-minutes?include_prereleases&color=yellow">
</a>
    <br>
    <h3>
    <br>
    Open Source • Privacy-First • Enterprise-Ready
    </h3>
    <p align="center">
    Get latest <a href="https://www.zackriya.com/meetily-subscribe/"><b>Product updates</b></a> <br><br>
    <a href="https://meetily.ai"><b>Website</b></a> •
    <a href="https://www.linkedin.com/company/106363062/"><b>LinkedIn</b></a> •
    <a href="https://discord.gg/crRymMQBFH"><b>Meetily Discord</b></a> •
    <a href="https://discord.com/invite/vCFJvN4BwJ"><b>Privacy-First AI</b></a> •
    <a href="https://www.reddit.com/r/meetily/"><b>Reddit</b></a>
</p>
    <p align="center">

A privacy-first AI meeting assistant that captures, transcribes, and summarizes meetings entirely on your infrastructure. Built by expert AI engineers passionate about data sovereignty and open source solutions. Perfect for enterprises that need advanced meeting intelligence without compromising on privacy, compliance, or control.

</p>

<p align="center">
    <img src="docs/meetily_demo.gif" width="650" alt="Meetily Demo" />
    <br>
    <a href="https://youtu.be/6FnhSC_eSz8">View full Demo Video</a>
</p>

</div>

---

> **Meetily PRO Available** - Looking for enhanced accuracy, advanced exports, custom summary workflows, team workflows, and managed compliance? Meetily PRO is the professional-grade solution for teams and organizations. **This Community Edition remains forever free & open source**. Use coupon code **LAUNCH20** for **20% off** until the next Meetily Community Edition release. Speaker diarization is planned for PRO in mid-June. [Learn more about PRO →](https://meetily.ai/pro/)

---

<details>
<summary>Table of Contents</summary>

- [Introduction](#introduction)
- [Why Meetily?](#why-meetily)
- [Features](#features)
- [Installation](#installation)
- [Key Features in Action](#key-features-in-action)
- [System Architecture](#system-architecture)
- [For Developers](#for-developers)
- [Meetily PRO](#meetily-pro)
- [Contributing](#contributing)
- [License](#license)

</details>

## Introduction

Meetily is a privacy-first AI meeting assistant that runs entirely on your local machine. It captures your meetings, transcribes them in real-time, and generates summaries, all without sending any data to the cloud. This makes it the perfect solution for professionals and enterprises who need to maintain complete control over their sensitive information.

## Why Meetily?

While there are many meeting transcription tools available, this solution stands out by offering:

- **Privacy First:** All processing happens locally on your device.
- **Cost-Effective:** Uses open-source AI models instead of expensive APIs.
- **Flexible:** Works offline and supports multiple meeting platforms.
- **Customizable:** Self-host and modify for your specific needs.

<details>
<summary>The Privacy Problem</summary>

Meeting AI tools create significant privacy and compliance risks across all sectors:

- **$4.4M average cost per data breach** (IBM 2024)
- **€5.88 billion in GDPR fines** issued by 2025
- **400+ unlawful recording cases** filed in California this year

Whether you're a defense consultant, enterprise executive, legal professional, or healthcare provider, your sensitive discussions shouldn't live on servers you don't control. Cloud meeting tools promise convenience but deliver privacy nightmares with unclear data storage practices and potential unauthorized access.

**Meetily solves this:** Complete data sovereignty on your infrastructure, zero vendor lock-in, and full control over your sensitive conversations.

</details>

## Features

- **Local First:** All processing is done on your machine. No data ever leaves your computer.
- **Real-time Transcription:** Get a live transcript of your meeting as it happens.
- **AI-Powered Summaries:** Generate summaries of your meetings using powerful language models.
- **Chat with Meetings:** Ask questions about a meeting directly from the meeting details UI using its transcript, summary, and context.
- **Microsoft 365 Calendar Integration:** Connect your calendar, fetch upcoming events, and attach current event context to recordings.
- **Automatic Meeting Detection:** Detect active meeting apps/windows and optionally auto-start or auto-stop recording.
- **Formatted Exports:** Export complete meetings to Markdown, DOCX, or PDF with metadata, summaries, and transcript segments.
- **Custom Transcription Servers:** Use OpenAI-compatible transcription endpoints, including audio and chat/vision APIs for models such as VibeVoice.
- **Speaker Labels:** Display speaker labels returned by transcription providers, including `SPEAKER_ID`-style labels.
- **Theme Sync:** Choose light, dark, or system theme; system mode follows OS appearance changes.
- **Multi-Platform:** Works on macOS, Windows, and Linux.
- **Open Source:** Meetily is open source and free to use.
- **Flexible AI Provider Support:** Choose from Ollama (local), Claude, Groq, OpenRouter, or use your own OpenAI-compatible endpoint.

## Installation

### 🪟 **Windows**

1. Download the latest `x64-setup.exe` from [Releases](https://github.com/Zackriya-Solutions/meeting-minutes/releases/latest)
2. Run the installer

### 🍎 **macOS**

1. Download `meetily_0.4.0_aarch64.dmg` from [Releases](https://github.com/Zackriya-Solutions/meeting-minutes/releases/latest)
2. Open the downloaded `.dmg` file
3. Drag **Meetily** to your Applications folder
4. Open **Meetily** from Applications folder

### 🐧 **Linux**

Build from source following our detailed guides:

- [Building on Linux](docs/building_in_linux.md)
- [General Build Instructions](docs/BUILDING.md)

**Quick start:**

```bash
git clone https://github.com/Zackriya-Solutions/meeting-minutes
cd meeting-minutes/frontend
pnpm install
./build-gpu.sh
```

## Key Features in Action

### 🎯 Local Transcription

Transcribe meetings entirely on your device using **Whisper** or **Parakeet** models. No cloud required.

<p align="center">
    <img src="docs/home.png" width="650" style="border-radius: 10px;" alt="Meetily Demo" />
</p>

### 📥 Import & Enhance `Beta`

Import existing audio files to generate transcripts, or enhance to re-transcribe any recorded meeting with a different model or language, all processed locally.

> Contributed by [Jeremi Joslin](https://github.com/jeremi), improved by [Vishnu P S](https://github.com/p-s-vishnu) and [Mohammed Safvan](https://github.com/mohammedsafvan)

<p align="center">
    <img src="docs/meetily-export.gif" width="650" style="border-radius: 10px;" alt="Import and Enhance" />
</p>

### 🤖 AI-Powered Summaries

Generate meeting summaries with your choice of AI provider. **Ollama** (local) is recommended, with support for Claude, Groq, OpenRouter, and OpenAI.

<p align="center">
    <img src="docs/summary.png" width="650" style="border-radius: 10px;" alt="Summary generation" />
</p>

<p align="center">
    <img src="docs/editor1.png" width="650" style="border-radius: 10px;" alt="Editor Summary generation" />
</p>

### 📆 Calendar Integration

Connect Microsoft 365 calendar, fetch upcoming events, and use the current calendar event as recording context so summaries can include the meeting title, schedule, and relevant event details.

### 🎥 Automatic Meeting Detection

Detect meeting activity from supported meeting apps/windows and optionally start or stop recording automatically. Meeting detection can be configured from Settings and works with the local recording workflow.

### 📄 Meeting Exports

Export complete meetings as **Markdown**, **DOCX**, or **PDF**, including meeting metadata, AI summaries, and transcript segments. Exported summaries preserve common Markdown formatting such as headings, lists, tables, and action items.

### 🌓 Theme Sync

Choose light, dark, or system theme. In system mode, Meetily follows operating system appearance changes so the app stays aligned with your desktop.

### 💬 Chat with Meetings

Ask questions about a meeting from the meeting details UI. Meetily uses the meeting transcript, generated summary, and additional context to answer follow-up questions without requiring you to manually search the full conversation.

### 🔒 Privacy-First Design

All data stays on your machine. Transcription models, recordings, and transcripts are stored locally.

<p align="center">
    <img src="docs/settings.png" width="650" style="border-radius: 10px;" alt="Local Transcription and storage" />
</p>

### 🌐 Custom OpenAI Endpoint Support

Use your own OpenAI-compatible endpoint for AI summaries and transcription. Transcription endpoints can use a standard audio API or a chat/vision API, which is useful for multimodal transcription models such as VibeVoice.

When a transcription provider returns speaker metadata, Meetily carries it through to the transcript UI. Supported response fields include `speaker`, `speaker_id`, `speakerId`, `speaker_label`, and `speakerLabel`, so labels such as `SPEAKER_00` can appear directly in transcripts and exports.

<p align="center">
    <img src="docs/custom.png" width="650" style="border-radius: 10px;" alt="Custom OpenAI Endpoint Configuration" />
</p>

### 🎙️ Professional Audio Mixing

Capture microphone and system audio simultaneously with intelligent ducking and clipping prevention.

<p align="center">
    <img src="docs/audio.png" width="650" style="border-radius: 10px;" alt="Device selection" />
</p>

### ⚡ GPU Acceleration

Built-in support for hardware acceleration across platforms:

- **macOS**: Apple Silicon (Metal) + CoreML
- **Windows/Linux**: NVIDIA (CUDA), AMD/Intel (Vulkan)

Automatically enabled at build time - no configuration needed.

## System Architecture

Meetily is a single, self-contained application built with [Tauri](https://tauri.app/). It uses a Rust-based backend to handle all the core logic, and a Next.js frontend for the user interface.

For more details, see the [Architecture documentation](docs/architecture.md).

## For Developers

If you want to contribute to Meetily or build it from source, you'll need to have Rust and Node.js installed. For detailed build instructions, please see the [Building from Source guide](docs/BUILDING.md).

### ⚙️ Deployment Configuration

Meetily can be preconfigured with a `deployment_config.json` file for seeded or managed deployments. Place this file in the Tauri app data directory, not the repository root.

On Windows, this is typically:

```text
%APPDATA%\com.meetily.ai\deployment_config.json
```

The config supports `summary`, `transcription`, and `calendar` sections. Each section can use:

- `mode: "seed"` to apply only when the setting is missing
- `mode: "managed"` to enforce the setting whenever deployment config is applied

Example summary configuration using a Custom OpenAI-compatible endpoint:

```json
{
  "version": 1,
  "summary": {
    "mode": "managed",
    "provider": "custom-openai",
    "model": "llama-3.1-8b-instruct",
    "apiKeyEnv": "CUSTOM_OPENAI_API_KEY",
    "customOpenAI": {
      "endpoint": "http://localhost:8000/v1",
      "maxTokens": 4096,
      "temperature": 0.2,
      "topP": 0.95
    }
  }
}
```

For Custom OpenAI summary configuration, only `provider`, `model`, and `customOpenAI.endpoint` are required. `apiKey`, `apiKeyEnv`, `maxTokens`, `temperature`, and `topP` are optional. If both `apiKeyEnv` and `apiKey` are provided, the environment variable wins.

Supported summary providers:

```text
builtin-ai, ollama, openai, claude, groq, openrouter, custom-openai
```

## Meetily Pro

<p align="center">
    <img src="docs/pv2.1.png" width="650" style="border-radius: 10px;" alt="Upcoming version" />
</p>

**Meetily PRO** is a professional-grade solution with enhanced accuracy and advanced features for serious users and teams. Built on a different codebase with superior transcription models and enterprise-ready capabilities.

### Community Thank-You Offer

Meetily Community Edition will remain free and open source. PRO exists for users and teams who want a more advanced meeting workflow, including higher transcription accuracy, custom summary templates, advanced exports, auto-meeting detection, and self-hosted deployment options.

For the community that helped Meetily grow, we are making the upgrade easier: use coupon code **LAUNCH20** for **20% off Meetily PRO** until the next Meetily Community Edition release.

Speaker diarization is planned for mid-June, bringing automatic speaker separation to PRO meetings.

### Key Advantages Over Community Edition:

- **Enhanced Accuracy**: Superior transcription models for professional-grade accuracy
- **Advanced Summary Customization**: Tailor summaries to your specific workflow and needs
- **Team Workflow Features**: Collaboration-oriented workflows for organizations
- **Enhanced Meeting Automation**: More advanced meeting joining and automation options
- **Advanced Speaker Workflows**: Organization-ready speaker analytics and review workflows *(Coming Soon)*
- **Advanced Meeting Chat**: Team-oriented meeting insights and shared query workflows
- **Advanced Calendar Workflows**: Team-focused calendar and scheduling capabilities
- **Self-Hosted Deployment**: Deploy on your own infrastructure for teams
- **GDPR Compliance Built-In**: Privacy by design architecture with complete audit trails
- **Priority Support**: Dedicated support for PRO users

### Who is PRO for?

- **Professionals** who need the highest accuracy for critical meetings
- **Teams and organizations** (2-100 users) requiring self-hosted deployment
- **Power users** who need advanced customization and workflow automation
- **Compliance-focused organizations** requiring GDPR readiness

> **Note:** Meetily Community Edition remains **free & open source forever** with local transcription, AI summaries, and core features. PRO is a separate professional solution for users who need enhanced accuracy and advanced capabilities.

For organizations needing 100+ users or managed compliance solutions, explore [Meetily Enterprise](https://meetily.ai/enterprise/).

**Learn more about pricing and features:** [https://meetily.ai/pro/](https://meetily.ai/pro/)

## Contributing

We welcome contributions from the community! If you have any questions or suggestions, please open an issue or submit a pull request. Please follow the established project structure and guidelines. For more details, refer to the [CONTRIBUTING.md](CONTRIBUTING.md) file.

Thanks for all the contributions. Our community is what makes this project possible.

## License

MIT License - Feel free to use this project for your own purposes.

## Acknowledgments

- We borrowed some code from [Whisper.cpp](https://github.com/ggerganov/whisper.cpp).
- We borrowed some code from [Screenpipe](https://github.com/mediar-ai/screenpipe).
- We borrowed some code from [transcribe-rs](https://crates.io/crates/transcribe-rs).
- Thanks to **NVIDIA** for developing the **Parakeet** model.
- Thanks to [istupakov](https://huggingface.co/istupakov/parakeet-tdt-0.6b-v3-onnx) for providing the **ONNX conversion** of the Parakeet model.

## Star History

[![Star History Chart](https://api.star-history.com/chart?repos=Zackriya-Solutions/meetily&type=date&legend=top-left)](https://www.star-history.com/?repos=Zackriya-Solutions%2Fmeetily&type=date&legend=bottom-right)
