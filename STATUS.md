# STATUS.md - Analýza projektu Muxy

**Datum:** 2026-01-13
**Stav:** Aktivní vývoj (WIP commity)
**Velikost:** ~6,700 řádků Rust kódu, 25 zdrojových souborů

---

## Přehled priorit

| Priorita | Kategorie | Počet položek |
|----------|-----------|---------------|
| P0 | Kritické bugy / Bezpečnost | 3 |
| P1 | Architektonická vylepšení | 5 |
| P2 | UX vylepšení | 8 |
| P3 | Nové features | 10 |
| P4 | Kvalita kódu | 7 |

---

## P0 - Kritické (opravit ihned)

### 1. Race conditions v PTY threadech
**Soubor:** `src/terminal/pty_manager.rs`

Reader/writer thready drží reference na kanály, ale nejsou explicitně awaited. Pokud PTY crashne, thready zůstanou viset navždy.

**Řešení:**
- Přidat cancellation token
- Ukládat JoinHandle do PtyHandle
- Implementovat graceful shutdown

### 2. Unsafe path traversal v layoutu
**Soubor:** `src/workspace/state.rs`

`LayoutNode::get_at_path()` a `get_at_path_mut()` používají rekurzi bez bounds checking. Poškozená cesta může způsobit panic.

```rust
// Současný stav - může panikovat
pub fn get_at_path_mut(&mut self, path: &[usize]) -> Option<&mut LayoutNode>
```

**Řešení:** Přidat validaci nebo vracet `Option<&mut LayoutNode>` konzistentně

### 3. Workspace auto-save performance
**Soubor:** `src/workspace/persistence.rs`

Synchronní JSON serializace při každé změně workspace. Žádný debouncing - rychlé split/close operace způsobují file thrashing.

**Řešení:**
- Implementovat debounced save (300-500ms delay)
- Async write operace
- Dirty flag pattern

---

## P1 - Architektonická vylepšení

### 1. Duplikovaná logika pro název terminálu
**Soubory:** `project_column.rs`, `terminal_pane.rs`, `sidebar.rs`

Všechny tři implementují identickou logiku: `custom → OSC title → ID prefix`

**Řešení:** Extrahovat do helper metody v `Terminal` nebo `Workspace`

```rust
impl Terminal {
    pub fn display_name(&self) -> String {
        self.custom_name
            .or(self.osc_title)
            .unwrap_or_else(|| self.id[..8].to_string())
    }
}
```

### 2. Race conditions v detached window tracking
**Soubor:** `src/app.rs`

`opened_detached_windows` HashSet je porovnáván s workspace state. Změna workspace během otevírání okna může způsobit duplicity.

**Řešení:** Používat workspace state jako jediný source of truth

### 3. Chybí error boundaries
**Více souborů**

- Selhání vytvoření terminálu pouze loguje, nezobrazuje user feedback
- PTY spawn errors tiše ignorovány
- Žádný recovery/retry mechanismus

**Řešení:**
- Toast notifikace pro chyby
- Retry tlačítko v terminal pane
- Graceful degradation

### 4. Input handling nekonzistence
**Soubor:** `src/elements/terminal_element.rs`

Key input prochází jak `on_key_down` tak `InputHandler::replace_text_in_range()`. Speciální klávesy bypass InputHandler, textové klávesy bypass KeyDown.

**Řešení:** Unifikovat input pipeline

### 5. Render performance - unbounded element creation
**Soubory:** `root.rs`, `split_pane.rs`

Project dividers a split handles jsou re-created každý render. Žádná memoizace.

**Řešení:**
- Cachovat statické elementy
- Implementovat shouldComponentUpdate pattern

---

## P2 - UX vylepšení

### 1. Keyboard navigace mezi terminály
Chybí Alt+1/2/3 pro přepínání projektů, Tab pro focus dalšího terminálu

### 2. Terminal preview na hover
V sidebaru zobrazit náhled příkazu/cwd při hoveru nad terminálem

### 3. Activity indicator
Vizuální indikátor aktivity v minimalizovaném/taskbar terminálu

### 4. Layout templates
Přednastavené layouty (např. "3-column dev setup", "2x2 grid")

### 5. Drag & drop pro tabs
Tab UI nemá drag-to-reorder funkcionalitu

### 6. Scroll position preservation
Pozice scrollu se nezachovává při přepínání mezi taby

### 7. Copy with formatting
Možnost kopírovat s ANSI formátováním

### 8. Settings UI
Grafické nastavení pro keybindings, barvy, font

---

## P3 - Nové features

### 1. Scrollback buffer
**Priorita:** Vysoká

Aktuálně žádná historie/backscroll - omezeno na viewport. Implementovat ring buffer pro historii.

### 2. Regex search
**Priorita:** Vysoká

Search podporuje pouze case-insensitive plaintext. Přidat regex matching.

### 3. Search ve scrollback
**Priorita:** Vysoká

Search prohledává pouze viditelnou obrazovku, ne historii.

### 4. Session persistence
**Priorita:** Střední

PTY procesy se neukládají mezi restarty aplikace. Implementovat session save/restore.

### 5. SSH integration
**Priorita:** Střední

Přímá podpora pro SSH connections s uloženými profily.

### 6. Split keyboard shortcuts
**Priorita:** Střední

Cmd+Arrow pro navigaci mezi split panes.

### 7. Terminal profiles
**Priorita:** Nízká

Uložené profily s různými shell, env variables, working directory.

### 8. Command palette
**Priorita:** Nízká

Cmd+Shift+P pro rychlý přístup ke všem akcím.

### 9. Plugin system
**Priorita:** Nízká

Rozšiřitelnost přes plugin API.

### 10. Broadcast input
**Priorita:** Nízká

Posílat input do více terminálů současně (pro cluster management).

---

## P4 - Kvalita kódu

### 1. Nepoužívané API
- `hidden_terminals` map existuje ale není používána
- Exit code je zachycen v `PtyEvent` ale není využit
- Scrollbar UI je rezervováno v theme ale nerendrováno
- Status indicators barvy definovány ale nepoužity

### 2. TODO komentář
```rust
// src/views/split_pane.rs:50
// Legacy alias - TODO: update callers to use init_drag_context directly
pub fn init_split_drag_context(cx: &mut App) { init_drag_context(cx) }
```

### 3. Neúplné features
- Select All v context menu nefunguje (komentář: "complex in terminal")
- Tab container základní - chybí reorder, close na middle click

### 4. Magic numbers
Mnoho hardcoded hodnot (velikosti, timeouty, barvy) by mělo být v constants

### 5. Test coverage
Žádné unit testy, integration testy, nebo benchmarky

### 6. Documentation
Chybí rustdoc komentáře pro public API

### 7. Error messages
Některé error messages jsou příliš technické, nejsou user-friendly

---

## Současný stav implementace

### Fungující features
- Multi-terminal management s nezávislými PTY procesy
- Rekurzivní split panes (horizontal/vertical)
- Tab kontejnery (základní)
- Drag-to-resize pro všechny split typy
- Minimize/restore terminály
- Fullscreen mode
- Detach do separátního okna
- Dark/Light theme s auto-detection
- Sidebar s navigací
- Text selection (single/double/triple click)
- Copy/paste clipboard integrace
- Search s highlighting
- Context menu
- Custom naming pro projekty/terminály
- JSON persistence

### Work in progress (z git diff)
- Terminal element search UI integrace
- Font variant caching (bold/italic)
- Search match structure
- Terminal rendering optimalizace

---

## Doporučený postup

1. **Týden 1:** Opravit P0 issues (race conditions, path validation, debounced save)
2. **Týden 2:** Refaktoring duplicitního kódu, error boundaries
3. **Týden 3:** UX vylepšení - keyboard nav, activity indicators
4. **Týden 4:** Scrollback buffer implementace
5. **Ongoing:** Nové features dle priority

---

## Technický dluh

| Oblast | Stav | Dopad |
|--------|------|-------|
| Testy | Žádné | Vysoký - regrese nejsou odhaleny |
| Dokumentace | Minimální | Střední - onboarding nových vývojářů |
| Error handling | Částečné | Vysoký - špatná UX při chybách |
| Performance | Dobrý | Nízký - ale může se zhoršit s růstem |

---

## Závěr

Muxy je dobře navržený terminálový multiplexer s solidní architekturou. Hlavní priority by měly být:

1. **Stabilita:** Opravit race conditions a přidat proper error handling
2. **UX:** Scrollback buffer je klíčová chybějící feature
3. **Kvalita:** Přidat testy před dalším rozvojem

Codebase je čistý a dobře strukturovaný, což usnadňuje budoucí rozvoj.
