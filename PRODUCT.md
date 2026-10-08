# Product

<!-- impeccable:product-schema 1 -->

## Platform

Native desktop: Windows, macOS, and Linux, using GPUI Kit. The desktop GPUI guides in `.agents/skills/` are authoritative; mobile/web conventions do not apply.

## Product purpose

Rustickers is a lightweight local desktop sticker app for text notes, timers, command output, file previews, and sketches. Stickers can be open on the desktop or closed without being deleted. Command schedules can run while their windows are closed.

## Operating context

The manager organizes saved stickers; transient TXTs and selection popups support short keyboard-driven workflows in another application. The app and CLI share SQLite storage. See README.md for usage and protection limitations.

## Capabilities and constraints

- Existing Markdown, Command, Timer, Paint, and File sticker types; no new text/vault product.
- One master-password system, per-action decryption, public searchable titles, and no recovery/global unlock session.
- Exact clipboard text and argument data must survive display and dispatch.
- The manager should expose useful object state without executing commands implicitly or creating competing editors.
- Four global hotkeys are editable in Settings with validation and durable local storage; quick file preview also supports a modifier-only release gesture. Escape dismissal is reserved. Hooks cannot guarantee conflict-free operation with other applications.
- Use temporary databases for development verification, never live user data.

## Brand commitments

The user requested a reimagined manager that is pretty, fast, simple, restrained, logical, and ergonomic (简洁高级). The user delegated design and implementation decisions to the assistant; suggested settings and dedicated command views are references rather than requirements.

## Evidence on hand

README.md, FRONTEND.md, existing native screens, and the supplied current-manager screenshot. No commercial claims or usage metrics. Audience is not confirmed beyond desktop users of the documented workflows.
