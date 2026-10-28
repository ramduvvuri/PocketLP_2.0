import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export interface PocketItem {
  item_id: string;
  item_type: "TEXT" | "LINK" | "NOTE" | "FILE" | "IMAGE";
  title: string;
  text_content?: string;
  url?: string;
  mime_type?: string;
  byte_size?: number;
  content_hash?: string;
  blob_status: "NONE" | "PENDING" | "VERIFIED";
  folder_id?: string;
  is_trashed: boolean;
  is_protected: boolean;
  has_conflict: boolean;
  aliases: string[];
  updated_at: number;
  last_used_at: number;
}

export interface ImageBoardItem {
  board_item_id: string;
  title: string;
  byte_size: number;
  content_hash: string;
  blob_status: string;
  captured_at: number;
  expires_at: number;
}

export interface CleanupCandidate {
  item_id: string;
  title: string;
  item_type: string;
  byte_size?: number;
  last_used_at: number;
  selected_for_deletion: boolean;
}

export default function App() {
  const [view, setView] = useState<"POCKET" | "FILES" | "IMAGE_BOARD" | "TRASH">("POCKET");
  const [items, setItems] = useState<PocketItem[]>([]);
  const [boardItems, setBoardItems] = useState<ImageBoardItem[]>([]);
  const [searchQuery, setSearchQuery] = useState("");
  const [captureTitle, setCaptureTitle] = useState("");
  const [captureBody, setCaptureBody] = useState("");
  const [syncLabel, setSyncLabel] = useState("Offline Â· 0 changes pending");
  const [syncStateKind, setSyncStateKind] = useState<"offline" | "syncing" | "synced" | "error">("offline");
  const [showPairModal, setShowPairModal] = useState(false);
  const [manualEndpoint, setManualEndpoint] = useState("192.168.1.100:7700");
  const [pairingCode, setPairingCode] = useState("842-910");
  const [showCleanupModal, setShowCleanupModal] = useState(false);
  const [cleanupCandidates, setCleanupCandidates] = useState<CleanupCandidate[]>([]);

  const refreshData = async () => {
    try {
      const fetchedItems = await invoke<PocketItem[]>("cmd_list_items", {
        search: searchQuery || null
      });
      setItems(fetchedItems);

      const fetchedBoard = await invoke<ImageBoardItem[]>("cmd_list_image_board");
      setBoardItems(fetchedBoard);

      const statusText = await invoke<string>("cmd_get_sync_status_label");
      setSyncLabel(statusText);
      if (statusText.startsWith("Synced")) setSyncStateKind("synced");
      else if (statusText.startsWith("Syncing")) setSyncStateKind("syncing");
      else if (statusText.startsWith("Error")) setSyncStateKind("error");
      else setSyncStateKind("offline");

      const candidates = await invoke<CleanupCandidate[]>("cmd_list_cleanup_candidates");
      setCleanupCandidates(candidates);
    } catch {
      // Fallback preview mode if running in pure browser dev
    }
  };

  useEffect(() => {
    refreshData();
  }, [searchQuery, view]);

  const handleCaptureTextOrLink = async () => {
    if (!captureBody.trim() && !captureTitle.trim()) return;
    const trimmed = captureBody.trim();
    const isUrl = trimmed.startsWith("http://") || trimmed.startsWith("https://");
    await invoke("cmd_create_text_item", {
      title: captureTitle.trim() || (isUrl ? trimmed : trimmed.slice(0, 48)),
      content: trimmed,
      isLink: isUrl
    });
    setCaptureTitle("");
    setCaptureBody("");
    await refreshData();
  };

  const handleTrashToggle = async (item: PocketItem) => {
    await invoke("cmd_set_item_trashed", {
      itemId: item.item_id,
      trashed: !item.is_trashed
    });
    await refreshData();
  };

  const handlePermanentDelete = async (itemId: string) => {
    await invoke("cmd_permanently_delete_item", { itemId });
    await refreshData();
  };

  const toggleCleanupSelection = (id: string) => {
    setCleanupCandidates((prev) =>
      prev.map((c) =>
        c.item_id === id ? { ...c, selected_for_deletion: !c.selected_for_deletion } : c
      )
    );
  };

  const confirmCleanupDecision = async () => {
    const selectedDelete = cleanupCandidates
      .filter((c) => c.selected_for_deletion)
      .map((c) => c.item_id);
    const deselectedProtect = cleanupCandidates
      .filter((c) => !c.selected_for_deletion)
      .map((c) => c.item_id);

    await invoke("cmd_apply_cleanup_decision", {
      selectedForDeleteIds: selectedDelete,
      deselectedProtectForeverIds: deselectedProtect
    });
    setShowCleanupModal(false);
    await refreshData();
  };

  const visibleItems = items.filter((item) => {
    if (view === "TRASH") return item.is_trashed;
    if (item.is_trashed) return false;
    if (view === "FILES") return item.item_type === "FILE";
    return true;
  });

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand-row">
          <span className="brand-title">PocketLP</span>
          <span className="badge">v1.0</span>
        </div>
        <button
          className={`nav-btn ${view === "POCKET" ? "active" : ""}`}
          onClick={() => setView("POCKET")}
        >
          <span>Pocket (Everything)</span>
          <span className="badge">{items.filter((i) => !i.is_trashed).length}</span>
        </button>
        <button
          className={`nav-btn ${view === "FILES" ? "active" : ""}`}
          onClick={() => setView("FILES")}
        >
          <span>Files</span>
          <span className="badge">
            {items.filter((i) => !i.is_trashed && i.item_type === "FILE").length}
          </span>
        </button>
        <button
          className={`nav-btn ${view === "IMAGE_BOARD" ? "active" : ""}`}
          onClick={() => setView("IMAGE_BOARD")}
        >
          <span>Image Board (12h)</span>
          <span className="badge">{boardItems.length}</span>
        </button>
        <button
          className={`nav-btn ${view === "TRASH" ? "active" : ""}`}
          onClick={() => setView("TRASH")}
        >
          <span>Trash</span>
          <span className="badge">{items.filter((i) => i.is_trashed).length}</span>
        </button>

        <div style={{ marginTop: "auto", display: "flex", flexDirection: "column", gap: "8px" }}>
          {cleanupCandidates.length > 0 && (
            <button className="btn secondary" onClick={() => setShowCleanupModal(true)}>
              14-Day Review ({cleanupCandidates.length})
            </button>
          )}
          <button className="btn secondary" onClick={() => setShowPairModal(true)}>
            Pair / Sync Diagnostics
          </button>
        </div>
      </aside>

      <main className="main-area">
        <header className="topbar">
          <input
            className="search-input"
            placeholder="Keyword search (SQLite FTS5) across titles, notes, URLs, and file aliases..."
            value={searchQuery}
            onChange={(e) => setSearchQuery(e.target.value)}
          />
          <button className="status-pill" onClick={() => setShowPairModal(true)}>
            <span className={`status-dot ${syncStateKind}`} />
            <span>{syncLabel}</span>
          </button>
        </header>

        {view !== "IMAGE_BOARD" && view !== "TRASH" && (
          <section className="capture-bar">
            <input
              className="capture-input"
              placeholder="Optional title..."
              value={captureTitle}
              onChange={(e) => setCaptureTitle(e.target.value)}
            />
            <input
              className="capture-input"
              placeholder="Capture note, code snippet, or paste URL (Ctrl+Space quick capture)..."
              value={captureBody}
              onChange={(e) => setCaptureBody(e.target.value)}
              onKeyDown={(e) => e.key === "Enter" && handleCaptureTextOrLink()}
            />
            <button className="btn" onClick={handleCaptureTextOrLink}>
              Save to Pocket
            </button>
          </section>
        )}

        {view === "IMAGE_BOARD" ? (
          <section className="cards-grid">
            {boardItems.map((b) => {
              const remainingHrs = Math.max(
                0,
                ((b.expires_at - Date.now()) / (1000 * 60 * 60)).toFixed(1)
              );
              return (
                <div key={b.board_item_id} className="item-card">
                  <div style={{ display: "flex", justifyContent: "space-between" }}>
                    <strong>{b.title}</strong>
                    <span className="badge">Expires in {remainingHrs}h</span>
                  </div>
                  <div style={{ fontSize: "0.8rem", color: "var(--text-secondary)" }}>
                    SHA-256: {b.content_hash.slice(0, 12)}... Â· Status: {b.blob_status}
                  </div>
                </div>
              );
            })}
          </section>
        ) : (
          <section className="cards-grid">
            {visibleItems.map((item) => (
              <div key={item.item_id} className="item-card">
                <div style={{ display: "flex", justifyContent: "space-between", gap: "6px" }}>
                  <strong>{item.title}</strong>
                  <div style={{ display: "flex", gap: "4px" }}>
                    <span className="badge">{item.item_type}</span>
                    {item.is_protected && <span className="badge protected">Keep Forever</span>}
                    {item.blob_status === "PENDING" && (
                      <span className="badge pending">Pending Blob</span>
                    )}
                  </div>
                </div>
                {item.text_content && (
                  <p style={{ fontSize: "0.86rem", color: "var(--text-secondary)" }}>
                    {item.text_content.slice(0, 160)}
                  </p>
                )}
                {item.aliases.length > 0 && (
                  <div style={{ fontSize: "0.75rem", color: "var(--text-secondary)" }}>
                    Aliases: {item.aliases.join(", ")}
                  </div>
                )}
                <div style={{ display: "flex", gap: "8px", marginTop: "6px" }}>
                  <button className="btn secondary" onClick={() => handleTrashToggle(item)}>
                    {item.is_trashed ? "Restore" : "Trash"}
                  </button>
                  {item.is_trashed && (
                    <button
                      className="btn danger"
                      onClick={() => handlePermanentDelete(item.item_id)}
                    >
                      Delete Permanently
                    </button>
                  )}
                </div>
              </div>
            ))}
          </section>
        )}
      </main>

      {showPairModal && (
        <div className="modal-backdrop" onClick={() => setShowPairModal(false)}>
          <div className="modal-box" onClick={(e) => e.stopPropagation()}>
            <h3>LAN Peer Pairing & Sync Diagnostics (_lp-sync._tcp : 7700)</h3>
            <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)" }}>
              Automatic mDNS discovery runs on private LAN/Hotspot. If router AP isolation blocks
              mDNS, enter the peer IP:Port directly below and verify the 6-digit TLS 1.3 fingerprint
              code on both devices.
            </p>
            <input
              className="capture-input"
              value={manualEndpoint}
              onChange={(e) => setManualEndpoint(e.target.value)}
              placeholder="Peer IP:Port (e.g., 192.168.43.1:7700)"
            />
            <div className="badge" style={{ alignSelf: "flex-start" }}>
              TLS 1.3 Short Verification Code: {pairingCode}
            </div>
            <div style={{ display: "flex", gap: "8px", justifyContent: "flex-end" }}>
              <button className="btn secondary" onClick={() => setShowPairModal(false)}>
                Close
              </button>
              <button
                className="btn"
                onClick={async () => {
                  await invoke("cmd_trigger_manual_sync", { endpoint: manualEndpoint });
                  setShowPairModal(false);
                  await refreshData();
                }}
              >
                Approve & Sync Now
              </button>
            </div>
          </div>
        </div>
      )}

      {showCleanupModal && (
        <div className="modal-backdrop" onClick={() => setShowCleanupModal(false)}>
          <div className="modal-box" onClick={(e) => e.stopPropagation()}>
            <h3>14-Day Inactivity Cleanup Review</h3>
            <p style={{ fontSize: "0.85rem", color: "var(--text-secondary)" }}>
              Checked items will be permanently deleted. Uncheck any item to mark it{" "}
              <strong>Protected / Keep Forever</strong> across both devices.
            </p>
            <div style={{ maxHeight: "260px", overflowY: "auto", display: "grid", gap: "8px" }}>
              {cleanupCandidates.map((c) => (
                <label
                  key={c.item_id}
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: "10px",
                    padding: "8px",
                    background: "rgba(255,255,255,0.04)",
                    borderRadius: "6px"
                  }}
                >
                  <input
                    type="checkbox"
                    checked={c.selected_for_deletion}
                    onChange={() => toggleCleanupSelection(c.item_id)}
                  />
                  <span>{c.title}</span>
                  <span className="badge" style={{ marginLeft: "auto" }}>
                    {c.selected_for_deletion ? "Delete" : "Protect Forever"}
                  </span>
                </label>
              ))}
            </div>
            <div style={{ display: "flex", gap: "8px", justifyContent: "flex-end" }}>
              <button className="btn secondary" onClick={() => setShowCleanupModal(false)}>
                Cancel
              </button>
              <button className="btn danger" onClick={confirmCleanupDecision}>
                Confirm Cleanup ({cleanupCandidates.filter((c) => c.selected_for_deletion).length}{" "}
                Delete / {cleanupCandidates.filter((c) => !c.selected_for_deletion).length} Protect)
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}