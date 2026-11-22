# Design

<!-- index: design | Visual direction, layout, keyboard map, and UX principles | before any UI work -->

Direction (owner brief, 2026-11-22): **minimal, keyboard-first, native macOS**.
The look is closer to Zed than to Chrome. The page gets almost the whole window,
the chrome is quiet, and every action is a shortcut or a ⌘K entry.

## Layout

```
┌──────────┬────────────────────────────────────────┬──────────────┐
│ Tabs     │                                        │ Inspector    │
│ (220 pt) │          WKWebView (active tab)        │ (⌘⌥I, 380pt) │
│ ● docs   │                                        │ Elements     │
│ ○ gh     │      split: ⌘\ adds a second pane      │ Network      │
│ ── Agent │                                        │ Console      │
│ ◆ shop   │                                        │ Cookies      │
├──────────┴────────────────────────────────────────┴──────────────┤
│ status: url · blocked count · agent connected                    │
└──────────────────────────────────────────────────────────────────┘
        ⌘K palette floats centred over everything (560 pt wide)
```

- **Tab bar:** a vertical list of title rows with favicon letter dots. The
  user's tabs come first, then an "Agent" group header with tinted rows for
  agent-owned tabs. Asleep tabs are dimmed. A tab waiting for a human shows ⚠.
- **Palette:** one input with ranked rows (open tabs, history, bookmarks,
  commands). The first row is always "Open <typed URL>" or "Search <query>".
- **Inspector:** a tab strip of four panels over monospace rows. Hovering an
  Elements row highlights the node in the page.

## Tokens

| Role | Dark | Light |
|---|---|---|
| background | `#1b1c1f` | `#f6f6f7` |
| surface (sidebar, panes) | `#232428` | `#ffffff` |
| border | `#34363b` | `#e2e2e6` |
| text | `#e6e6e9` | `#1c1c1f` |
| muted text | `#9a9ca3` | `#6b6d75` |
| accent (focus, active tab) | `#5b8cff` | `#2f6bff` |
| agent tint | `#b07cff` at 18% | `#7a3cff` at 12% |
| error | `#ff6b6b` | `#d93636` |
| warn | `#ffb84d` | `#b26b00` |

Type: system UI font at 13 pt for chrome and 12 pt monospace for the Inspector.
Corner radius is 6 pt and spacing uses a 4 pt grid. Motion is 150 ms, and is
none under Reduce Motion.

## Keyboard map

| Keys | Action |
|---|---|
| ⌘K | palette |
| ⌘L | palette prefilled with the current URL |
| ⌘T / ⌘W / ⌘⇧T | new tab / close tab / reopen closed tab |
| ⌘1–⌘9, ⌃Tab / ⌃⇧Tab | switch tab |
| ⌘[ / ⌘] / ⌘R | back / forward / reload |
| ⌘⌥I | toggle the native Inspector |
| ⌘⌥U | WebKit Web Inspector (fallback) |
| ⌘\ | split / unsplit the content area |
| f (page focused, no input) | link hints |
| Esc | close palette / hints |

## States

- Empty window: the welcome card (Import from Chrome, MCP setup command).
- Loading: a thin accent progress line under the toolbar.
- Load failure: an in-pane error with Retry.
- Asleep: the snapshot, dimmed.
- Inspector with no DOM yet: "Waiting for page…".
