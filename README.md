# SmartAPC

SmartAPC V1 ist eine modulare Regelungssoftware in Rust für OT-/Industrie-Umgebungen.

## Aktueller Status (Stand April 2026)

### Bereits umgesetzt

- Modulare Runtime mit klaren Interfaces:
  - `ProtocolAdapter` (V1: OPC-UA Client Adapter)
  - `WatchdogModule` (`none`, `counter_mirror`)
  - `ControlLogic`:
    - `p_controller` (produktiv nutzbar)
    - `linear_mpc` (lineares MPC auf Basis des gespeicherten State-Space-Modells)
    - `custom_rust` (projektlokale `cdylib`)
  - Persistiertes `linear_state_space`-Modell für Offline-Simulation
- Runtime-State-Machine:
  - `Running -> Hold -> Alarm -> Off`
  - Fault-Verhalten: Hold Last Value -> Alarm -> Off
  - Im `Off`-State werden keine Outputs geschrieben.
- Laufzeit-Logging in `projects/<projekt>/data/runtime.log` (JSONL):
  - Inputs/Outputs je Zyklus
  - Online-Parameter je Zyklus
  - Online-Parameter-Änderungen
  - User-definierte Logs aus `custom_rust`-Strategien und strukturierte `linear_mpc`-Diagnosen
- Simulations-Logging in `projects/<projekt>/data/simulation.log` (JSONL):
  - `simulation_cycle` je Zyklus
  - `simulation_time_s`
  - Inputs/Outputs und User-Logs analog zur Runtime
- Native Desktop-App (`iced`) mit Setup-Workflow:
  - OPC-UA Kommunikation
  - Watchdog
  - Ein-/Ausgangsnamen
  - I/O-Definition inkl. OPC Read/Write Test
  - Optionales State-Space-Modell-Setup (`linear_state_space`, A/B/C/D Import + Matrix-Editor)
  - Optionales Controller-Simulations-Setup mit Start/Stop, Geschwindigkeitswahl, Initialwerten, Störungen, Rauschen und Live-A/B-Skalierung
  - Regellogik-Reiter mit Strategiewahl
  - Control Function Tests mit Input-/Parameter-Sweeps, Fehler-Checks und Laufzeitmetriken
  - Online-Parameter (f64/bool), Runtime-Update + Persistenz
  - Statische I/O-Initialwerte (ohne OPC-Adresse) als Online-Parameter editierbar
  - Plotansicht aus lokaler SQLite
- Lokale Web-API:
  - Projekte laden/speichern
  - Plot-Konfiguration pro Projekt (Window, Reihenfolge, Custom Plots)
  - Runtime Start/Stop/State
  - Simulation Start/Stop/State
  - Aktive Execution-Reads für State, Parameter und Plots
  - Runtime-Parameter lesen/schreiben
  - Simulation Step
  - Plotdaten (Limit + Sliding Window + Series-Katalog)
  - Deployment-Kommandos (Windows Service via `sc.exe`)
- Interaktive Web-Plotansicht:
  - `GET /plots`
  - Folgt automatisch Runtime oder Simulation
  - Plotly.js lokal gebündelt (kein CDN notwendig)
  - Sliding-Window-Zeitfenster als User-Parameter
  - MV/CV/DV Panels per Drag-and-Drop sortierbar
  - Eigene Multi-Trace-Plots aus Online-Parametern und geloggten Variablen
- Virtuelle Hardware:
  - Separater lokaler OPC-UA PT1-Server
  - Popup-Liveplot mit 3-Minuten-Fenster

### Noch als Platzhalter (UI-seitig)

- Deployment-Setup-Seite (API-Kommandos vorhanden)

Hinweis zum Modell-Setup:

- Die Desktop-App kann ein optionales kontinuierliches Zustandsraummodell `linear_state_space` mit
  `x_dot = A x + B u` und `y = C x + D u` speichern.
- Die Offline-Reglersimulation verwendet dieses Modell direkt.
- Als Löser wird ein festes RK4-Verfahren mit Zero-Order Hold pro Regelzyklus verwendet.
- Die Runtime selbst verwendet weiterhin keine interne Pflanzen-Simulation.

## Build, Test, Start

```bash
cargo test
cargo run -- desktop
cargo run -- serve
cargo run -- virtual-hardware
```

Typischer Start der Desktop-App:

```bash
cargo run -- desktop --project-root ./projects
```

Desktop ohne eingebettete Web-API:

```bash
cargo run -- desktop --project-root ./projects --no-web
```

## Projektlayout

```text
projects/
  <project_name>/
    project.toml
    data/
      runtime.sqlite
      runtime.log
      simulation.sqlite
      simulation.log
    logic/
      custom_strategy/   # wird bei Template-Generierung angelegt
```

## Schnellstart mit virtueller Hardware

1. Virtuelle Hardware starten:

```bash
cargo run -- virtual-hardware --host 127.0.0.1 --port 4855 --dt-ms 100 --tau-s 5.0 --gain 1.0 --history 120
```

2. Desktop-App in zweitem Terminal starten:

```bash
cargo run -- desktop --project-root ./projects
```

3. In der App:
- Endpoint auf `opc.tcp://127.0.0.1:4855/` setzen.
- Watchdog konfigurieren:
  - `read_node = ns=2;s=wd_counter_read`
  - `write_node = ns=2;s=wd_counter_mirror`
- Variablen und I/O definieren (z. B. `cv.y1.*`, `mv.u1.*`).
- Regellogik auswählen und Runtime starten.

Virtuelle Hardware OPC-UA Tags:

- `ns=2;s=y1_value`
- `ns=2;s=y1_target`
- `ns=2;s=y1_HL`
- `ns=2;s=y1_LL`
- `ns=2;s=u1_write`
- `ns=2;s=u1_read`
- `ns=2;s=wd_counter_read`
- `ns=2;s=wd_counter_mirror`

## Regelstrategie in V1

Im Reiter **Regellogik** gibt es oben eine Strategiewahl.

- Basis: `P-Regler` (`p_controller`)
- Modellbasiert: `Linear MPC` (`linear_mpc`)
- Alternative: `Eigene Regellogik` (`custom_rust`)

Wichtig: Die Auswahl zeigt nur Strategien, die mit der aktuellen I/O-Definition kompatibel sind.  
`Linear MPC` erscheint, wenn ein `linear_state_space`-Modell sowie passende MV/CV/DV-I/O vorhanden sind.
`Eigene Regellogik` erscheint erst nach erfolgreichem Build/Check als kompatibel.

### P-Regler

- Explizite CV->MV Paarung
- Pro Paar:
  - `kp`
  - `bias`
- Formel: `u = kp * (soll - ist) + bias`
- Enable-Verhalten:
  - Falls `cv.<name>.enable == false` oder `mv.<name>.enable == false`, wird für dieses Paar kein Output geschrieben.

### Linear MPC

- Verwendet das gespeicherte kontinuierliche Modell `x_dot = A x + B u`, `y = C x + D u`.
- Diskretisierung erfolgt je Regelzyklus mit Zero-Order Hold/RK4 wie in der Offline-Simulation.
- `u_order` bleibt `MVs + DVs`: MVs werden optimiert, DVs werden als gemessene Störungen über den Horizont konstant gehalten.
- Benötigte I/O:
  - je CV: `cv.<name>.soll` und `cv.<name>.ist`
  - je MV: `mv.<name>.write`; `mv.<name>.read` ist optional und wird sonst durch den letzten Stellwert ersetzt
  - je DV: `dv.<name>.read`
- Online-Parameter:
  - Horizonte: `mpc.prediction_horizon`, `mpc.control_horizon`
  - CV-Tuning: `mpc.cv.<cv>.q`, `mpc.cv.<cv>.band_low`, `mpc.cv.<cv>.band_high`
  - MV-Tuning: `mpc.mv.<mv>.r`, `mpc.mv.<mv>.max_delta_u`
  - Kalman-Filter: `mpc.kalman.initial_p`, `mpc.kalman.state.<idx>.q`, `mpc.kalman.cv.<cv>.r`
- CV-Bänder sind relativ zum Live-Sollwert. Innerhalb des Bandes wird kein CV-Fehler bestraft; außerhalb wird auf die nächste Bandkante optimiert.
- MV-Grenzen kommen aus `io.mv.<mv>.ll` und `io.mv.<mv>.hl`, sofern statisch oder als OPC-Eingang verfügbar.
- Laufzeit-/Simulationslogs enthalten plotbare `user_log`-Serien wie `mpc.xhat.*`, `mpc.du.*`, `mpc.target.*`, `mpc.pred.free.*`, `mpc.pred.mpc.*` und `mpc.status.*`.

## Guide: Eigene Rust-Regellogik (`custom_rust`)

### Voraussetzungen

- Rust Toolchain lokal installiert (`cargo` verfügbar)
- Projekt-I/O bereits definiert
- Runtime gestoppt (für Konfig-Änderungen)
- Simulation gestoppt (für strukturelle Konfig-Änderungen)

### Schritt-für-Schritt

1. In der Desktop-App zum Reiter **Regellogik**.
2. Button **Template generieren** klicken.
   - Erzeugt unter `projects/<projekt>/logic/custom_strategy/` ein Rust-`cdylib`-Template.
   - Das Template ist ausführlich kommentiert (Context, History, Output-Pflicht, Logging-Helfer).
3. Datei `projects/<projekt>/logic/custom_strategy/src/lib.rs` bearbeiten.
4. Die Funktion `control(...)` implementieren.
5. Optional zusätzliche Parameter als Funktionsargumente hinter `ctx` und `out` ergänzen:
   - erlaubt: `f64`, `bool`
6. Button **Strategie prüfen/builden** klicken.
   - Führt `cargo build --release` für die Custom-Strategie aus.
   - Extrahiert zusätzliche Parameter aus der Signatur.
   - Liest den Contract aus der gebauten `cdylib` (`required_inputs`, `required_outputs`).
   - Markiert Strategie nur dann als kompatibel, wenn Contract und aktuelle I/O-Definition exakt übereinstimmen.
7. Jetzt kann im Auswahlmenü **Eigene Regellogik** gewählt und gespeichert werden.
8. Zusätzliche Parameter erscheinen unter **Online Parameter** und sind zur Laufzeit änderbar.

### Erwartete Signatur

Pflicht:

```rust
pub fn control(ctx: &SmartApcContext, out: &mut SmartApcOutputs)
```

Mit zusätzlichen User-Parametern:

```rust
pub fn control(
    ctx: &SmartApcContext,
    out: &mut SmartApcOutputs,
    gain: f64,
    enable_ff: bool,
)
```

Alle zusätzlichen Parameter werden als Online-Parameter registriert:

- `f64` -> numerischer Parameter
- `bool` -> boolescher Parameter

### Inputs/Outputs in Custom-Logik

- `ctx.inputs` enthält die aktuellen Runtime-Eingänge (aus I/O-Definition).
- `ctx.input_history` enthält Input-Verlauf je Signal (ältest -> neuest).
- `ctx.output_history` enthält Output-Verlauf je Signal (ältest -> neuest).
- `ctx.param_history` enthält Verlauf der Online-Parameter (ältest -> neuest).
- Über `out` müssen die benötigten `mv.<name>.write` Outputs gesetzt werden.
- Wenn ein erforderlicher Output nicht gesetzt wird, gilt die Strategie als fehlerhaft.

### Eigene Logs aus `control(...)`

Im Template stehen flexible Log-Helfer zur Verfügung:

- `out.log("text")`
- `out.log_value("text", "key", SignalValue::F64(...))`
- `out.log_values("text", values_map)`

Diese Einträge landen im Runtime-Logfile (`data/runtime.log`) im Feld `user_logs`.
Während der Offline-Simulation landen sie analog in `data/simulation.log`.

### Minimalbeispiel

```rust
pub fn control(
    ctx: &SmartApcContext,
    out: &mut SmartApcOutputs,
    kp: f64,
) {
    // Optional: Beispiel für User-Log
    out.log("control tick");

    let soll = match ctx.inputs.get("cv.y1.soll") {
        Some(SignalValue::F64(v)) => *v,
        _ => 0.0,
    };
    let ist = match ctx.inputs.get("cv.y1.ist") {
        Some(SignalValue::F64(v)) => *v,
        _ => 0.0,
    };
    out.set_f64("mv.u1.write", kp * (soll - ist));
}
```

### Online-Parameter aus statischen I/O-Werten

Wenn in der I/O-Definition ein Wert **ohne OPC-Adresse** konfiguriert ist, aber einen Initialwert hat,
wird er automatisch als Online-Parameter gespiegelt:

- `io.mv.<name>.hl`
- `io.mv.<name>.ll`
- `io.cv.<name>.sollband`

Damit können diese Werte auch zur Laufzeit über **Online Parameter** angepasst werden.

## Wichtige Hinweise

- V1 ist lokal/OT-fokussiert, ohne Cloud-Zwang.
- `runtime.cycle_ms` muss in V1 `>= 1000 ms` sein.
- Kommunikation:
  - `communication.endpoint_url`
  - `communication.io_cycle_ms`
- Watchdog `counter_mirror`:
  - `read_node`
  - `write_node`
  - `mirror_cycle_ms`
  - `stale_timeout_ms`

## API Kurzreferenz

- `GET /api/runtime/state`
- `GET /api/runtime/params`
- `PUT /api/runtime/params`
- `POST /api/simulation/start`
- `POST /api/simulation/stop`
- `GET /api/simulation/state`
- `GET /api/execution/state`
- `GET /api/execution/params`
- `GET /api/plots?limit=250`
- `GET /api/runtime/plots/window?window_ms=180000`
- `GET /api/runtime/plots/catalog`
- `GET /api/execution/plots/window?window_ms=180000`
- `GET /api/execution/plots/catalog`
- `GET /api/projects/{name}/plots/config`
- `PUT /api/projects/{name}/plots/config`
- `POST /api/simulate/step`
