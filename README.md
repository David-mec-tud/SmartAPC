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

## Build & Test

```bash
cargo test
cargo run -- serve
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

4. API nutzen:

- `GET /api/runtime/state`
- `GET /api/runtime/params`
- `PUT /api/runtime/params`
- `GET /api/plots?limit=250`

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

## OPC-UA Integrationstest

Der OPC-UA Adapter besitzt einen echten Client-Pfad (`opc.tcp://...`) und einen Mock-Pfad (`mock://...`) für Offline-Tests.

Der Test `real_opcua_connects_to_fake_plc_server` startet eine fiktive Steuerung als lokalen OPC-UA-Server und prüft:

- Aufbau der OPC-UA Verbindung
- Lesen periodisch aktualisierter Werte (z. B. Watchdog Counter / PV)
- Schreiben und Rücklesen von Stellgrößen (MV, Bool-Signal)
- Watchdog-Mirror-Write
