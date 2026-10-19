# Taskbar Monitor

Лёгкий виджет для панели задач Windows 10/11: загрузка **CPU / GPU / RAM / VRAM / сети / дисков**, температуры
и лимиты подписки **Claude Code** (сессия 5 ч, неделя 7 д и недельный лимит модели — Fable / Opus, если он есть
в вашем тарифе). Написан на Rust поверх чистого Win32, без рантаймов и фреймворков: один `exe` ≈ 6 МБ.

- **Всё настраивается**: какие метрики, в каком порядке, как выглядят (текст, полоса, вертикальная полоса,
  заливка, линейный график, график с заливкой, столбцы, кольцо), подписи, цвета, пороги, шрифт, отступы,
  число строк, положение на панели, мониторы.
- **Системная тема**: светлая/тёмная панель задач и акцентный цвет Windows подхватываются на лету;
  есть ручные темы и свои цвета.
- **Раскладки (пресеты)**: «Графики», «Компактный текст», «Минимализм», «Кольца», «Только Claude», «Плотно, 3 строки».
- **Окно подробностей** по клику: карточки с большими графиками и деталями (ГБ из ГБ, адаптер, время до сброса лимита).
- **Значок в трее**: меню, сводка в подсказке, опционально живое значение прямо в значке.
- **7 языков интерфейса**: English, Русский, 中文, Español, Français, Português, Deutsch; по умолчанию — язык Windows.
- **Два варианта поставки**: установщик (автозапуск от администратора без UAC) и portable (права администратора по кнопке).

## Установка

Скачайте последнюю сборку на странице [Releases](https://github.com/noartem/taskbar-monitor-widget/releases)
(или артефакты последнего прогона [Actions](https://github.com/noartem/taskbar-monitor-widget/actions)).
Есть сборки для **x64** и **ARM64**.

### Установщик — `TaskbarMonitor-<версия>-x64-setup.exe`

- Ставится в `Program Files` (нужны права администратора).
- Галочка **«Запускать вместе с Windows от имени администратора»** (включена по умолчанию) создаёт задание
  Планировщика `TaskbarMonitor` с наивысшими правами: при входе в систему виджет стартует **от администратора без
  запроса UAC**.
- Настройки — `%APPDATA%\TaskbarMonitor\config.toml`.
- Удаление через «Приложения» снимает задание автозапуска и ключ `Run`.

### Portable — `TaskbarMonitor-<версия>-x64-portable.zip`

- Распакуйте куда угодно и запустите `taskbar-monitor.exe`. Рядом лежит `portable.txt` — пока он есть, настройки и
  журнал хранятся в этой же папке.
- Стартует с **обычными правами**. Чтобы получить права администратора: ПКМ по виджету или значку в трее →
  **«Перезапустить от имени администратора…»** → подтвердить → принять UAC. Новая копия дождётся завершения старой
  и запустится с правами администратора.
- Автозапуск включается из меню трея: «от текущего пользователя» (ключ `Run`, без админа) или «от администратора»
  (задание Планировщика, один раз запросит UAC).

### Зачем права администратора

Без них работает почти всё. С ними дополнительно:
- старт при входе без UAC (через Планировщик заданий);
- температуры ACPI (`MSAcpi_ThermalZoneTemperature`), где счётчики термозон недоступны.

## Что и откуда берётся

| Метрика | Источник | Админ |
|---|---|---|
| CPU | счётчик `Processor Information\% Processor Utility` (как в Диспетчере задач), запасной путь — `GetSystemTimes` | нет |
| RAM | `GlobalMemoryStatusEx` | нет |
| GPU | счётчики `GPU Engine` — самый загруженный тип движка, как в Диспетчере задач | нет |
| VRAM | `GPU Adapter Memory\Dedicated Usage` + объём из DXGI | нет |
| Температура/вентилятор GPU | драйвер WDDM (`D3DKMT` perf data — то же, что показывает Диспетчер задач) | нет |
| Температура CPU | LibreHardwareMonitor (если запущен) → термозоны ACPI → WMI `MSAcpi` | для WMI |
| Любой датчик | WMI-мост к **LibreHardwareMonitor / OpenHardwareMonitor**: частоты, вентиляторы, напряжения, мощность… | нет |
| Сеть | счётчики интерфейсов IP Helper (`GetIfTable2`); по умолчанию только физические адаптеры | нет |
| Диск | счётчики `PhysicalDisk` (чтение, запись, активность), место — `GetDiskFreeSpaceEx` | нет |
| Claude Code | `GET https://api.anthropic.com/api/oauth/usage` с OAuth-токеном Claude Code | нет |

Собираются только данные, которые где-то показываются (виджет, окно подробностей или значок в трее).
Отрисовка — Direct2D/DirectWrite в программный буфер (без создания GPU-устройства) раз в интервал обновления.
Окно настроек (egui) — отдельный процесс, который запускается только когда вы его открываете.

### Claude Code

Виджет читает токен, который Claude Code (CLI) сохраняет после `/login` в `%USERPROFILE%\.claude\.credentials.json`
(или в `%CLAUDE_CONFIG_DIR%`), и раз в 2 минуты запрашивает те же лимиты, что показывает команда `/usage`:

- **5h** — текущая 5-часовая сессия;
- **7d** — недельный лимит по всем моделям;
- **модель** — недельный лимит конкретной модели (Fable, Opus, Sonnet…). По умолчанию выбирается
  автоматически (Fable → Opus → Sonnet) и **скрывается, если в тарифе такого лимита нет**. Новые лимиты,
  которые появятся в API, подхватываются без обновления программы;
- **$** — дополнительное (pay-as-you-go) использование, если включено.

Если Claude Code работает в WSL, укажите путь к файлу, например
`\\wsl$\Ubuntu\home\<user>\.claude\.credentials.json`. Токен отправляется только на адрес API использования
и не пишется в журнал. Если он истёк — достаточно один раз запустить `claude`. Кнопка «Проверить сейчас»
в настройках показывает все найденные лимиты.

## Управление

- **Клик** — окно подробностей, **двойной клик** — Диспетчер задач, **средняя кнопка** — настройки
  (всё переназначается), **ПКМ** — меню.
- Наведите на плитку — подсказка с деталями.
- Снимите «Закрепить положение» и перетащите виджет вдоль панели задач.
- Меню трея: подробности, настройки, пресеты и число строк, автозапуск, перезапуск от администратора,
  обновить данные Claude, открыть `config.toml`/папку, выход.

## Конфигурация

Всё, что есть в окне настроек, хранится в `config.toml` (TOML, можно править руками — изменения применяются на лету;
битый файл сохраняется как `config.toml.broken`, а виджет продолжает работать с настройками по умолчанию).

```toml
[general]
language = "auto"            # auto (язык Windows) | en | ru | zh | es | fr | pt | de
refresh_interval_ms = 1000
history_length = 60          # точек в графиках
autostart = "off"            # off | user | elevated
network_unit = "bytes"       # bytes | bits
left_click = "flyout"        # none | flyout | settings | toggle_widget | task_manager
double_click = "task_manager"
middle_click = "settings"

[appearance]
theme = "system"             # system | light | dark | custom
font_family = "Segoe UI Variable Text"
font_size = 0.0              # 0 = по высоте строки
background_opacity = 0.0     # 0 = прозрачный фон
use_system_accent = true
color_by_category = true
warn_threshold = 70.0
crit_threshold = 90.0

[layout]
placement = "near_tray"      # near_tray | left | center
offset_x = 0.0               # смещение вдоль панели (DIP)
rows = 2                     # 1..4
monitor = "primary"          # primary | all | specific (+ monitor_index)
lock_position = true
hide_on_fullscreen = true
embed = "overlay"            # overlay | embedded (экспериментально)

[tray]
show_icon = true
icon_mode = "app_icon"       # app_icon | metric
icon_metric = "cpu_usage"

[claude]
enabled = true
credentials_path = ""        # пусто = %USERPROFILE%\.claude\.credentials.json
poll_interval_secs = 120

[hardware]
gpu_sensors = true           # температура/вентилятор GPU из драйвера
thermal_zones = true         # термозоны ACPI
lhm_bridge = true            # датчики LibreHardwareMonitor через WMI
gpu_adapter = ""             # GPU по умолчанию (подстрока имени)

[[metrics]]                  # плитки в порядке отображения; заполняют столбцы сверху вниз
kind = "cpu_usage"
style = "line"               # text | bar | v_bar | fill | line | area | columns | ring
show_in_widget = true
show_in_flyout = true

[[metrics]]
kind = "claude_model"
source = "fable"             # пусто/auto = Fable → Opus → Sonnet
style = "ring"
overlay_text = true
hide_when_unavailable = true
```

Виды метрик (`kind`): `cpu_usage`, `cpu_temp`, `ram_usage`, `gpu_usage`, `gpu_temp`, `vram_usage`, `net_down`,
`net_up`, `net_total`, `disk_read`, `disk_write`, `disk_active`, `disk_space`, `claude_session`, `claude_weekly`,
`claude_model`, `claude_extra`, `sensor`.

Поле `source` у метрики: имя GPU; адаптер сети (`*` = все); диск (`0 C:`, пусто = все); буква тома (`D:`);
идентификатор датчика LibreHardwareMonitor (`/amdcpu/0/temperature/2`); модель Claude (`fable`, `opus`).
Прочие поля: `label`, `show_label`, `show_value`, `overlay_text`, `color` (`#RRGGBB`), `width`, `decimals`,
`max_value` (шкала для скоростей/датчиков, 0 = авто), `hide_when_unavailable`.

## Сборка

Нужен Rust (stable, ≥ 1.95) и Windows 10/11 SDK (Visual Studio Build Tools).

```powershell
cargo build --release -p taskbar-monitor
# target\release\taskbar-monitor.exe
```

Платформенно-независимое ядро (настройки, модель метрик, форматирование, разбор ответа Claude, локализация)
собирается и тестируется где угодно: `cargo test -p tm-core`. Проверить Windows-часть с Linux можно
кросс-компиляцией: `rustup target add x86_64-pc-windows-gnu` + `mingw-w64`, затем
`cargo build --target x86_64-pc-windows-gnu -p taskbar-monitor`.

Установщик собирается [Inno Setup 6](https://jrsoftware.org/isinfo.php):

```powershell
cd installer
iscc /DAppVersion=0.1.0 /DArch=x64 /DSourceExe=..\target\release\taskbar-monitor.exe taskbar-monitor.iss
```

### CI/CD

`.github/workflows/build.yml`:
- на каждый push/PR — `cargo fmt`, `clippy -D warnings`, тесты (Linux и Windows), сборка x64 и ARM64,
  portable-zip и установщик в артефактах прогона;
- на тег `v*` (например `git tag v1.0.0 && git push --tags`) — то же самое с версией из тега и публикация
  GitHub Release с zip, установщиками и `SHA256SUMS.txt`.

## Структура

```
crates/core   tm-core: config.toml, пресеты, модель метрик, резолвер, форматирование, Claude usage, i18n (ru/en)
crates/app    taskbar-monitor.exe:
  app.rs          цикл сообщений, клики/перетаскивание, хуки панели задач, применение настроек
  widget.rs       окно виджета на панели задач (overlay / embedded), подсказки
  tiles.rs        раскладка и отрисовка плиток
  flyout.rs       окно подробностей
  tray.rs         значок в трее, меню, живой значок
  render.rs       Direct2D/DirectWrite → layered window
  taskbar.rs      поиск панелей задач, трея, полноэкранных приложений
  collector/      сбор данных: PDH, DXGI/D3DKMT, IP Helper, WMI (LibreHardwareMonitor), Claude
  settings_ui.rs  окно настроек (egui, отдельный процесс)
  autostart.rs    ключ Run / задание Планировщика
  elevation.rs    проверка прав и перезапуск через UAC
installer/    Inno Setup + portable.txt
```

## Известные ограничения

- Windows 11 не даёт сторонним окнам резервировать место на панели задач: в режиме `overlay` виджет рисуется поверх
  свободной области (по умолчанию слева от трея). Если он перекрывает значки — сдвиньте его (`offset_x` или
  перетаскиванием) или выберите другое положение.
- Температура CPU без LibreHardwareMonitor доступна не на всех машинах (зависит от ACPI-термозон).
- API использования Claude не документирован публично; разбор устроен терпимо к изменениям, но формат может поменяться.
