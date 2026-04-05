# SmartAPC

SmartAPC V1 ist eine modulare Regelungssoftware in Rust für OT-/Industrie-Umgebungen.

## V1 Features

- Modulares Runtime-Design mit klaren Interfaces:
  - `ProtocolAdapter` (V1: `OpcUaClientAdapter`)
  - `WatchdogModule` (`none`, `counter_mirror`)
  - `ControlLogic` (V1: statisch kompilierte Mapping-Logik)
  - `ModelModule` (optional, V1: `NoopModel`)
- Runtime-State-Machine: `Running -> Hold -> Alarm -> Off`
- Fault-Verhalten:
  - Hold Last Value
  - Alarm nach konfigurierbarem Timeout
  - Off-State nach zweitem Timeout (keine Output-Schreibzugriffe)
- Projektkonfiguration als `TOML + Ordnerstruktur`
- Lokaler Webserver mit API für:
  - Projekt anlegen/laden/speichern
  - Sidebar-Aufgabenstatus
  - Runtime Start/Stop/State
  - Runtime-Parameter lesen/schreiben (inkl. Persistenz in TOML)
  - Simulation Step
  - Plot-Daten aus SQLite
  - Windows Service Deployment-Kommandos (`sc.exe`)
- Ausführungszeittest-Basismetriken:
  - Zyklusdauer
  - Jitter
  - Deadline-Miss-Zähler
- Native Desktop-Setup-App mit `iced`:
  - Projektanlage/-auswahl
  - Sidebar-Setup-Aufgaben
  - Runtime Start/Stop
  - Online-Parameter (ändern + persistieren)
  - Lokale Plotansicht aus SQLite
  - Optional eingebettete Web-API parallel zur Desktop-App

## Build & Test

```bash
cargo test
cargo run -- serve
cargo run -- desktop
cargo run -- virtual-hardware
```

## Schnellstart

1. Projekt anlegen:

```bash
cargo run -- init-project --name demo --project-root ./projects
```

2. `./projects/demo/project.toml` bearbeiten (I/O, Limits, Mapping-Regeln).
   Als Vorlage kannst du [`examples/demo_project.toml`](examples/demo_project.toml) nutzen.

3. Runtime + Webserver starten:

```bash
cargo run -- serve --project-root ./projects --project demo --host 127.0.0.1 --port 8080
```

Alternative: Native Desktop-App (inkl. optional eingebettetem Webserver):

```bash
cargo run -- desktop --project-root ./projects --host 127.0.0.1 --port 8080
```

Webserver im Desktop-Modus deaktivieren:

```bash
cargo run -- desktop --project-root ./projects --no-web
```

4. API nutzen:

- `GET /api/runtime/state`
- `GET /api/runtime/params`
- `PUT /api/runtime/params`
- `GET /api/plots?limit=250`

## Virtuelle Hardware (separater OPC-UA PT1 Server)

Für unabhängige End-to-End-Tests kannst du eine lokale, separate PT1-Hardware-Simulation starten:

```bash
cargo run -- virtual-hardware --host 127.0.0.1 --port 4855 --dt-ms 100 --tau-s 5.0 --gain 1.0 --history 120
```

Die virtuelle Hardware stellt diese OPC-UA-Tags bereit:

- `ns=2;s=y1_value`
- `ns=2;s=y1_target`
- `ns=2;s=y1_HL`
- `ns=2;s=y1_LL`
- `ns=2;s=u1_write`
- `ns=2;s=u1_read`

Zusätzliche Watchdog-Testtags:

- `ns=2;s=wd_counter_read` (läuft hoch)
- `ns=2;s=wd_counter_mirror` (Schreibziel)

Beim Start öffnet sich außerdem ein Plot-Fenster (Popup):

- y-Plot: `y1_value`, `y1_target`, `y1_HL`, `y1_LL`
- u-Plot: `u1_write/u1_read`
- Zeitfenster: immer die letzten `3 Minuten` (fix)

Beispielprojekt für diese Hardware:

- [`examples/virtual_pt1_project.toml`](examples/virtual_pt1_project.toml)

Watchdog-Test gegen die virtuelle Hardware:

1. Virtuelle Hardware in Terminal A starten (`cargo run -- virtual-hardware ...`).
2. In Terminal B SmartAPC starten (`cargo run -- desktop` oder `cargo run -- serve --project ...`).
3. Endpoint auf `opc.tcp://127.0.0.1:4855/` setzen.
4. Watchdog konfigurieren:
   - `read_node = ns=2;s=wd_counter_read`
   - `write_node = ns=2;s=wd_counter_mirror`
   - z. B. `mirror_cycle_ms = 1000`, `stale_timeout_ms = 5000`
5. In der Desktop-App auf der Watchdog-Seite `Test` ausführen und dann Runtime starten.

## Projektlayout

```text
projects/
  <project_name>/
    project.toml
    data/
      runtime.sqlite
```

## Hinweise

- V1 enthält eine trait-basierte OPC-UA-Adapterstruktur mit einer austauschbaren Client-Implementierung.
- Für produktive Feldintegration kann der Adapter intern gegen einen vollständigen OPC-UA-Transport ausgetauscht werden, ohne Runtime-/Web-API-Brüche.
- Watchdog `counter_mirror` nutzt:
  - `read_node` (OPC-UA Tagname Quelle)
  - `write_node` (OPC-UA Tagname Spiegelziel)
  - `mirror_cycle_ms` (Intervall für Read -> Write)
  - `stale_timeout_ms` (Fault, wenn Read-Wert nicht mehr wechselt)
- Kommunikationsbasis (`[communication]`):
  - `endpoint_url` (OPC-UA Serveradresse)
  - `io_cycle_ms` (Read/Write Intervall für I/O-Kommunikation)

## OPC-UA Integrationstest

Der OPC-UA Adapter besitzt einen echten Client-Pfad (`opc.tcp://...`) und einen Mock-Pfad (`mock://...`) für Offline-Tests.

Der Test `real_opcua_connects_to_fake_plc_server` startet eine fiktive Steuerung als lokalen OPC-UA-Server und prüft:

- Aufbau der OPC-UA Verbindung
- Lesen periodisch aktualisierter Werte (z. B. Watchdog Counter / PV)
- Schreiben und Rücklesen von Stellgrößen (MV, Bool-Signal)
- Watchdog-Mirror-Write
