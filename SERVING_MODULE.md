# Moduł Serving - Dokumentacja

## Przegląd

Nowy moduł `serving` umożliwia zbieranie danych systemowych i wyświetlanie ich w czasie rzeczywistym przez przeglądarkę.

## Architektura

### 1. **DataBuffer** (`serving/data_buffer.rs`)
- Bufor cykliczny przechowujący ostatnie N wyników (domyślnie 10, max 1000)
- Thread-safe dzięki `Arc<RwLock<VecDeque>>`
- Automatyczne usuwanie najstarszych danych po przekroczeniu limitu

### 2. **HTTP Server** (`serving/server.rs`)
- Serwer HTTP na porcie 5998 (domyślnie)
- Endpointy:
  - `GET /` - interfejs HTML z tabelą danych
  - `GET /api/data` - JSON z wszystkimi danymi
  - `GET /api/stats` - JSON ze statystykami
- Auto-refresh co 1 sekundę

### 3. **Serve Collector** (`serving/serve_collector.rs`)
- Zbiera dane systemowe (CPU, pamięć, procesy)
- Wysyła do `DataBuffer` zamiast zapisywać do pliku
- Działa w osobnym wątku - nie blokuje serwera

## Użycie

### Podstawowe uruchomienie:
```bash
cargo run -- serve
```

### Z opcjami:
```bash
# Uruchom na porcie 8080, przechowuj 50 wyników, auto-otwórz przeglądarkę
cargo run -- serve -p 8080 -l 50 -o

# Monitoruj Firefox i zbieraj co 0.5s
cargo run -- serve -e "FIREFOX|firefox" -c 0.5

# Wszystkie opcje
cargo run -- serve \
  -c 0.5 \              # interwał zbierania (0.5s)
  -p 5998 \             # port serwera
  -l 100 \              # max wyników (1-1000)
  -o \                  # otwórz przeglądarkę
  -e "FIREFOX|firefox" \# monitoruj proces
  -m cpu-usage-total memory-used  # zbieraj CPU i pamięć
```

## Parametry CLI

- `-c, --check-interval <INTERVAL>` - Interwał zbierania w sekundach (domyślnie: 1.0)
- `-p, --port <PORT>` - Port HTTP serwera (domyślnie: 5998)
- `-l, --max-results <MAX_RESULTS>` - Max liczba wyników (1-1000, domyślnie: 10)
- `-o, --open-browser` - Automatycznie otwórz przeglądarkę
- `-e, --process-cmd-to-search <CMD>` - Monitoruj proces (format: "NAZWA|szukany_tekst")
- `-m, --collection-mode <DATA_TYPE>` - Typy danych do zbierania

## Interfejs WWW

### Tabela danych pokazuje:
- Timestamp (czas lokalny)
- CPU Usage (%) - jeśli zbierane
- Memory Used (MB) - jeśli zbierane
- Memory Available (MB) - jeśli zbierane
- Swap Used (MB) - jeśli zbierane
- Custom Processes - monitorowane procesy z CPU i pamięcią

### Auto-refresh:
- Co 1 sekundę pobiera nowe dane
- Pokazuje tylko ostatnie N wyników (ustawione przez -l)
- Lekkie zapytania GET do /api/data

## Architektura wątków

```
Main Thread
├── Server Task (tokio::spawn)
│   ├── HTTP Server (Axum)
│   └── Obsługa requestów
│
└── Data Collection Loop
    ├── Refresh system data
    ├── Push to DataBuffer
    └── Ctrl+C handling
```

## Różnice vs tryb Collect

| Feature | Collect | Serve |
|---------|---------|-------|
| Zapis do pliku | ✅ | ❌ |
| Backup | ✅ | ❌ |
| Convert after | ✅ | ❌ |
| Real-time view | ❌ | ✅ |
| HTTP Server | ❌ | ✅ |
| Limit danych | Rozmiar pliku | Liczba rekordów |

## Zależności

Dodane do `Cargo.toml`:
- `axum = "0.8"` - Framework HTTP
- `serde_json = "1.0"` - Serializacja JSON
- `tower-http = { version = "0.6", features = ["fs", "trace"] }` - HTTP utilities

## Przykłady

### 1. Monitoring CPU i pamięci
```bash
cargo run -- serve -m cpu-usage-total memory-used -c 0.5 -o
```

### 2. Monitoring Firefox z historią 100 wyników
```bash
cargo run -- serve -e "FIREFOX|firefox" -l 100 -o
```

### 3. Długoterminowe zbieranie na niestandardowym porcie
```bash
cargo run -- serve -p 8080 -l 1000 -c 2.0
```

## Zakończenie

Naciśnij **Ctrl+C** aby zatrzymać serwer i zbieranie danych.

