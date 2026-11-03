# Wdrożenie Jarvisa

WinUI i Android budujesz na swoim komputerze. Czat i `/admin` to statyczny build Vite, który Caddy serwuje na tej samej nazwie co rdzeń. Mózg, baza, SearXNG, terminal i ewentualny pulpit zostają na maszynie OCI.

Kolejność: najpierw VM, na końcu sparowanie Windowsa i telefonu. Stronę otwierasz pod adresem API.

## OCI

Maszyna to Always Free Ampere A1, ARM64. Obraz w `docker-compose.yml` w katalogu głównym jest ustawiony na `linux/arm64`. Porty 80 i 443 mają być otwarte w liście zabezpieczeń VCN. Port 8080 zostaje wewnątrz sieci Dockera.

Na VM:

```bash
sudo apt update
sudo apt install -y docker.io docker-compose-v2 git
sudo usermod -aG docker "$USER"
```

Wyloguj się i zaloguj ponownie, sklonuj repozytorium i wejdź do jego katalogu głównego. Tam musi leżeć `docker-compose.yml`. Compose czyta plik tylko z bieżącego katalogu. Uruchomienie z innego miejsca kończy się komunikatem `no configuration file provided: not found`.

```bash
git clone <adres-repozytorium> JarvisAgent
cd JarvisAgent
ls docker-compose.yml
```

W tym samym katalogu utwórz plik `.env`:

```bash
JARVIS_DOMAIN=jarvis.twoja-domena.pl
JARVIS_WEB_ORIGIN=https://jarvis.twoja-domena.pl
JARVIS_COOKIE_SECURE=1
JARVIS_WEB_PASSWORD=dlugie-haslo
JARVIS_PAIR_CODE=jednorazowy-kod
GROQ_API_KEY=
GEMINI_API_KEY=
OPENROUTER_API_KEY=
```

`JARVIS_DOMAIN` to nazwa, na którą Caddy wystawi HTTPS dla strony, API i WebSocket. Ta zmienna musi wejść do kontenera Caddy. W `.env` ma być sama nazwa, bez `https://`, na przykład `gawr-jarvis.duckdns.org`. Rekord A w DuckDNS wskazuje publiczny adres VM. Porty 80 i 443 są otwarte w liście zabezpieczeń VCN i w zaporze samej VM. Bez portu 80 Caddy nie dostanie certyfikatu Let's Encrypt, a `curl` kończy się błędem TLS `internal error`. `JARVIS_WEB_ORIGIN` to ten sam adres, razem z `https://` i bez ukośnika na końcu. `JARVIS_COOKIE_SECURE=1` jest wymagane, bo publiczna strona idzie po HTTPS. Hasło web nie zostawiaj puste: bez tej zmiennej rdzeń przyjmuje hasło `jarvis`.

Klucze Groq, Gemini i OpenRouter są opcjonalne w tym pliku. Da się je też wpisać później w `/admin`. Sync Turso zostaje wyłączony, dopóki w panelu nie włączysz flagi `turso_sync` i nie podasz `TURSO_DATABASE_URL` oraz tokenu.

Uruchomienie:

```bash
docker compose up -d --build
docker compose ps
curl -fsS https://jarvis.twoja-domena.pl/health
```

Odpowiedź `ok` znaczy, że Caddy doszedł do rdzenia. Wolumen `jarvis-data` trzyma `jarvis.db`, `server.key`, `jarvis.env` i vault. Katalog startowy terminala w panelu to `/app`.

Pulpit OCI nie startuje razem ze stosem. W `/admin` przycisk pulpitu woła profil `desktop` tylko wtedy, gdy na VM zostaje co najmniej 1,5 GB wolnego RAM. VNC słucha wtedy na `127.0.0.1:6080`.

Strażnik, gdy ma sprawdzać publiczną nazwę, dostaje `JARVIS_PUBLIC_HOST`. To może być ten sam host co Caddy albo nazwa tunelu Cloudflare albo DuckDNS.

Aktualizacja z GitHuba: push na `main`, który dotyka `apps/core`, `vendor/branches`, `deploy`, `docker-compose.yml` albo `.dockerignore`, uruchamia workflow `Deploy OCI`. Runner tylko wchodzi po SSH na VM. Obraz buduje się na Ampere, z lokalnego cache Dockera.

W repozytorium GitHub, w Settings → Secrets and variables → Actions, dodaj:

| Sekret | Wartość |
| --- | --- |
| `OCI_HOST` | publiczny adres VM, IP albo nazwa DuckDNS |
| `OCI_USER` | użytkownik SSH, zwykle `ubuntu` |
| `OCI_SSH_KEY` | prywatny klucz, którego publiczna połowa jest w `~/.ssh/authorized_keys` na VM |

Katalog `~/JarvisAgent` na VM ma być klonem `origin/main` bez lokalnych zmian w śledzonych plikach. `.env` zostaje na miejscu, bo nie jest w gicie. Jeśli repozytorium jest prywatne, na VM dodaj deploy key z prawem odczytu, inaczej `git pull` w workflow się nie powiedzie.

Ręcznie, z katalogu głównego, ten sam skrypt:

```bash
cd ~/JarvisAgent
git pull --ff-only
bash deploy/oci-up.sh
```

Dockerfile składa obraz w dwóch krokach. Najpierw kompiluje zależności na pustym `main.rs`. Ta warstwa zostaje, dopóki nie zmienią się `Cargo.toml`, `Cargo.lock` albo `vendor/branches`. Drugi krok dokłada prawdziwe źródła i kompiluje sam `jarvis-core`. Rejestr crate'ów jest w cache BuildKita, więc ponowne pobieranie też odpada. Profil release ma `opt-level` 2 i wyłączone LTO, żeby Ampere kończył ten drugi krok szybko. Pierwsza budowa po tej zmianie i tak przejdzie wszystkie skrzynki. `docker builder prune` kasuje ten zysk do następnej pełnej kompilacji.

`deploy/oci-up.sh` buduje `core` i Caddy ze stroną, podnosi SearXNG i czeka, aż `http://127.0.0.1:8080/health` wewnątrz kontenera rdzenia zwróci odpowiedź.

## Strona

Caddy serwuje `apps/web` z tego samego kontenera, który kończy TLS. `/api` i `/health` idą do rdzenia. Każda inna ścieżka, której nie ma na dysku, dostaje `index.html`. Budowa nie dostaje `VITE_API_URL`, więc czat i WebSocket zostają na hoście strony.

Na VM ustaw `JARVIS_WEB_ORIGIN` na `https://` plus wartość `JARVIS_DOMAIN`. Potem przebuduj stos:

```bash
cd ~/JarvisAgent
bash deploy/oci-up.sh
```

Wejście na stronę to jedno pole hasła. Wartość to `JARVIS_WEB_PASSWORD` z VM. `/admin` używa tego samego hasła i nie przenosi rozmowy.

Lokalny podgląd strony, z rdzeniem na `http://localhost:8080`:

```powershell
copy .env.example .env
copy apps\web\.env.example apps\web\.env
cd apps\web
pnpm install
pnpm dev
```

`.env` w katalogu głównym steruje rdzeniem przy `cargo run`. Poza Dockerem wartości z tego pliku wygrywają z `data/jarvis.env`, więc panel ich nie utrwala między restartami. Puste linie i komentarze są pomijane. Plik nie wchodzi do gita. `apps/web/.env` ustawia `VITE_API_URL` dla Vite. Origin zostaje `http://localhost:5173`, a `JARVIS_COOKIE_SECURE` wyłączone.

## WinUI

Budowa jest lokalna, na Windowsie. Potrzebny jest .NET 10 SDK i Windows App SDK, które dociąga projekt.

```powershell
cd apps\desktop
dotnet build -c Release -p:Platform=x64
```

Wynik to niepakowany program `JarvisDesktop.exe`. Modele głosu kładziesz obok exe, w katalogu `models`:

- `models\hey_jarvis.onnx`
- `models\whisper\` z modelem Whisper
- `models\male_voxcpm_2.onnx`
- `models\matcha.exe`

Gdy któregoś pliku nie ma, pasek zostaje przy tekście.

W oknie programu wpisujesz adres API, na przykład `https://jarvis.twoja-domena.pl`, i kod z `JARVIS_PAIR_CODE`. Sparowanie zapisuje token urządzenia przez DPAPI. Identyfikator tego komputera to `desktop-pc`.

## Android

Budowa jest lokalna w Android Studio. Otwórz katalog `apps\mobile`. SDK 35, JDK 17. W repozytorium nie ma wrappera Gradle. Android Studio wygeneruje go przy pierwszym otwarciu, albo z katalogu modułu:

```powershell
cd apps\mobile
gradle wrapper
.\gradlew.bat :app:assembleDebug
```

APK debug leży w `apps\mobile\app\build\outputs\apk\debug\`. Na telefonie włącz instalację z tego źródła. Do pływającego paska potrzebne jest pozwolenie rysowania nad innymi aplikacjami, a do skrótu powiadomień dostęp do powiadomień.

W aplikacji wpisujesz ten sam adres API i ten sam kod parowania. Identyfikator telefonu to `android-phone`. Na emulatorze host komputera to `http://10.0.2.2:8080`.

Modele głosu na telefonie to ten sam głos Matcha, w int8. Dopóki ich nie ma, pasek pokazuje tekst.
