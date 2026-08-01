import { Search } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { useRampage } from "../store";

const commands = [
  { name: "Run a proof job", description: "Issue a lease and execute a harmless echo task", action: "run" },
  { name: "Pool a proof across devices", description: "Preview and run three independent shards with signed receipts", action: "pool" },
  { name: "Explain current work", description: "Open accessible nodes and evidence", action: "explain" },
  { name: "Replay the evidence spine", description: "Refresh the ordered ledger worldline", action: "replay" },
  { name: "Invite a device", description: "Generate a ten-minute one-time enrollment code", action: "invite" },
] as const;

export function CommandPalette() {
  const state = useRampage();
  const [query, setQuery] = useState("");
  const [busy, setBusy] = useState(false);
  const input = useRef<HTMLInputElement>(null);
  useEffect(() => {
    const listener = (event: KeyboardEvent) => {
      if ((event.ctrlKey || event.metaKey) && event.key.toLowerCase() === "k") {
        event.preventDefault(); state.setCommandOpen(!state.commandOpen);
      }
      if (event.key === "Escape") state.setCommandOpen(false);
    };
    window.addEventListener("keydown", listener);
    return () => window.removeEventListener("keydown", listener);
  }, [state.commandOpen, state.setCommandOpen]);
  useEffect(() => { if (state.commandOpen) input.current?.focus(); }, [state.commandOpen]);
  if (!state.commandOpen) return null;
  const matches = commands.filter(({ name, description }) => `${name} ${description}`.toLowerCase().includes(query.toLowerCase()));
  const choose = async (action: typeof commands[number]["action"]) => {
    setBusy(true);
    try {
      if (action === "run") await state.runDemo();
      if (action === "pool") await state.runPoolProof();
      if (action === "invite") await state.createInvite();
      if (action === "replay") await state.refresh();
      if (action === "explain") state.setMode("grid");
      state.setCommandOpen(false);
    } finally {
      setBusy(false);
    }
  };
  return (
    <div className="palette-backdrop" role="presentation" onMouseDown={() => state.setCommandOpen(false)}>
      <section className="palette" role="dialog" aria-modal="true" aria-label="Command portal" onMouseDown={(event) => event.stopPropagation()}>
        <label><Search size={19} /><input ref={input} value={query} onChange={(event) => setQuery(event.target.value)} placeholder="Ask Rampage or choose an action…" /></label>
        <div role="listbox">{matches.map(({ name, description, action }) => <button key={name} role="option" disabled={busy} onClick={() => void choose(action)}><strong>{name}</strong><span>{description}</span></button>)}</div>
        <footer><kbd>Esc</kbd> close <kbd>Enter</kbd> select <span>AI proposes · Governor decides</span></footer>
      </section>
    </div>
  );
}
