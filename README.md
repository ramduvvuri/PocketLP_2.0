# PocketLP 2.0 (Windows 11 x64 + Android 16 ARM64)

Local-first personal Pocket shared between **Lenovo LOQ (Windows 11 x64)** and **iQOO Z9 5G (Android 16, ARM64)** over trusted Wi-Fi or Hotspot without cloud servers.

## Features Implemented
- **Unified Local-First Pocket:** Capture `TEXT`, `LINK`, `NOTE`, `FILE`, and `IMAGE` offline or online.
- **Content-Addressed SHA-256 Deduplication:** Identical bytes share one blob and record alternate filenames as searchable aliases.
- **LAN Peer Sync (`_lp-sync._tcp` port `7700`):** mDNS discovery + Manual IP/QR fallback + TLS 1.3 fingerprint pinning.
- **Resumable 4 MB Chunked File Transfers:** Supports 100 MB â€“ 1 GB files with `.partial` offset recovery and SHA-256 verification.
- **Image Board (12-Hour Ephemeral Screenshots):** Separate namespace that expires strictly 12 hours after original capture time (`captured_at + 12h`).
- **14-Day Inactivity Cleanup Review:** Candidates default to selected for deletion; unchecking an item marks it **Protected / Keep Forever** across both devices.