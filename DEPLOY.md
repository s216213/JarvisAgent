# Wdrożenie Jarvisa

WinUI i Android budujesz na swoim komputerze. Na Vercel idzie tylko statyczny czat i `/admin`. Mózg, baza, SearXNG, terminal i ewentualny pulpit zostają na maszynie OCI.

Kolejność: najpierw VM, potem Vercel, na końcu sparowanie Windowsa i telefonu. Adres strony i adres API muszą do siebie pasować, zanim otworzysz logowanie z internetu.

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
JARVIS_WEB_ORIGIN=https://twoj-projekt.vercel.app
JARVIS_COOKIE_SECURE=1
JARVIS_WEB_PASSWORD=dlugie-haslo
JARVIS_PAIR_CODE=jednorazowy-kod
GROQ_API_KEY=
GEMINI_API_KEY=
OPENROUTER_API_KEY=
```

`JARVIS_DOMAIN` to nazwa, na którą Caddy wystawi HTTPS dla API i WebSocket. `JARVIS_WEB_ORIGIN` to dokładny adres produkcji z Vercel, razem z `https://` i bez ukośnika na końcu. `JARVIS_COOKIE_SECURE=1` jest wymagane, bo strona i API są na różnych hostach. Hasło web nie zostawiaj puste: bez tej zmiennej rdzeń przyjmuje hasło `jarvis`.

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

Aktualizacja, nadal z katalogu głównego repozytorium:

```bash
cd ~/JarvisAgent
git pull
docker compose up -d --build
```

## Vercel

Na Vercel wjeżdża wyłącznie `apps/web`. Katalog główny projektu ustaw na `apps/web`. Menedżer pakietów to pnpm. Polecenie budowania zostaje domyślne: `pnpm build`. `vercel.json` w tym katalogu już przepisuje ścieżki na `index.html`.

Jedna zmienna środowiska, dostępna przy budowaniu:

```text
VITE_API_URL=https://jarvis.twoja-domena.pl
```

To jest adres Caddy, nie adres Vercel. Czat, ciasteczko, WebSocket, terminal i pulpit idą prosto na OCI. Vercel nie może ich proxować.

Po pierwszym wdrożeniu skopiuj dokładny adres strony do `JARVIS_WEB_ORIGIN` na VM i przebuduj rdzeń:

```bash
cd ~/JarvisAgent
docker compose up -d --build core
```

Wejście na stronę to jedno pole hasła. Wartość to `JARVIS_WEB_PASSWORD` z VM. `/admin` używa tego samego hasła i nie przenosi rozmowy.

Lokalny podgląd strony, z rdzeniem na `http://localhost:8080`:

```powershell
cd apps\web
pnpm install
pnpm dev
```

Wtedy origin to `http://localhost:5173`, a `JARVIS_COOKIE_SECURE` zostaje wyłączone.

## WinUI

Budowa jest lokalna, na Windowsie. Potrzebny jest .NET 8 SDK i Windows App SDK, które dociąga projekt.

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
