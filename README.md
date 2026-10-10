# Download Manager – The Ultimate IDM & Ghost Downloader Alternative

Welcome to **Download Manager**, an extremely fast, high-performance, and modern open-source download manager designed to replace legacy tools like **IDM (Internet Download Manager)** and serve as a powerful **Ghost Downloader alternative**.

Whether you're downloading massive files over HTTP/HTTPS or fetching high-quality media from YouTube, Download Manager offers blazing-fast speeds, dynamic segmentation, and a sleek, modern UI built with Tauri and React.

## 🚀 Why Choose Download Manager? (The Best IDM Alternative)

For years, Internet Download Manager (IDM) has been the industry standard, but it has grown bloated, outdated, and restricted to Windows platforms with paid licenses. Ghost Downloader provided a solid free alternative, but it lacks the robust modern UI and seamless browser integration required by today's power users.

**Download Manager solves this by offering:**
- **Blazing Fast Speeds:** Utilizes an advanced Rust-based chunked downloading engine to dynamically segment files (up to 32 concurrent connections), maximizing your bandwidth just like IDM.
- **100% Free & Open Source:** No annoying license pop-ups, no trial periods, and no hidden telemetry.
- **Modern UI/UX:** Built with Tauri, React, and Tailwind CSS, featuring a beautiful dark-mode interface, glassmorphism, and smooth animations.
- **Native YouTube Integration:** Powered by `yt-dlp` under the hood to automatically probe, extract, and download high-quality YouTube videos and audio flawlessly.
- **Browser Extension Support:** Seamlessly captures downloads from your browser directly into the app.

## ✨ Key Features

- **Dynamic File Segmentation:** Intelligently splits downloads into multiple parts (e.g., 16 or 32 segments) to drastically improve download speeds on throttled networks.
- **Robust Pause & Resume:** Safely pause your downloads and resume them anytime without data corruption, thanks to a robust `.part` metadata tracking system.
- **Advanced Network Stack:** Built on `reqwest` and `tokio`, featuring native root certificates for seamless TLS/HTTPS handshakes on modern operating systems.
- **Adaptive Speed Limiting:** Allows you to cap download speeds globally if you need bandwidth for other applications.
- **Cross-Platform Foundation:** The core engine is built in Rust, paving the way for seamless operation across Windows, macOS, and Linux.

## 📥 Installation

1. Head over to our [Releases](https://github.com/engrshoaibh/Download-Manager/releases) page.
2. Download the latest `.exe` (or `.msi`) installer for your platform.
3. Install and run! The app will run quietly in the background and can be summoned via the system tray.

## 🛠️ Building from Source

If you want to compile Download Manager yourself, you'll need **Rust**, **Node.js**, and **Tauri CLI**.

```bash
# Clone the repository
git clone https://github.com/engrshoaibh/Download-Manager.git
cd Download-Manager

# Install UI dependencies
cd ui
npm install

# Run in development mode
npm run tauri dev
```

## 🤝 Contributing

We welcome contributions! If you want to help make this the best IDM alternative in the world, feel free to fork the repository, open issues, and submit pull requests.

## 📝 License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for more information.

---
*Keywords for search visibility: Free IDM alternative, Internet Download Manager replacement, Ghost Downloader alternative, open source download manager, fast download manager, Rust download manager, YouTube video downloader.*
