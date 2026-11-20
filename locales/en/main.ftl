## Hyper Clips — English
app-name = Hyper Clips

## Tray
tray-tooltip = Hyper Clips — replay { $replay }
tray-open = Open Hyper Clips
tray-save-clip = Save clip
tray-toggle-recording = Start/stop recording
tray-screenshot = Take screenshot
tray-quit = Quit
state-on = on
state-off = off

## Actions (also used for hotkey labels)
action-save-clip = Save clip
action-toggle-recording = Start/stop recording
action-screenshot = Screenshot
action-add-marker = Add marker
action-open-studio = Open Hyper Clips

## Notifications
notify-unavailable = { $action }: not available ({ $reason }).
notify-clip-saved = Clip saved: { $seconds } s — { $game }
notify-recording-started = Recording started
notify-recording-stopped = Recording saved ({ $duration }, { $markers } markers)
notify-marker = Marker added
notify-screenshot = Screenshot saved
notify-error = Error: { $error }
notify-hotkey-conflicts = Some hotkeys could not be registered. Open settings to fix them.
notify-already-running = Hyper Clips is already running.

## Studio navigation
nav-library = Library
nav-editor = Editor
nav-settings = Settings
nav-privacy = Privacy
placeholder-library = The clip library arrives in Phase 3.
placeholder-editor = The editor arrives in Phase 4.

## Settings
settings-general = General
settings-language = Language
settings-language-auto = Automatic (Windows)
settings-theme = Theme
theme-light = Light
theme-dark = Dark
theme-oled = OLED black
settings-autostart = Start with Windows
settings-autostart-portable = Not available in portable mode.
settings-capture = Capture
settings-replay-enabled = Instant Replay enabled
settings-replay-seconds = Replay length (seconds)
settings-fps = Frame rate
settings-bitrate = Bitrate (kbps)
settings-hotkeys = Hotkeys
settings-hotkeys-hint = Examples: F8, Ctrl+Shift+S, Alt+Mouse5. Leave empty to disable.
settings-updates = Updates
settings-update-check = Check for updates
update-on-startup = At startup
update-every-hours = Every N hours
update-never = Never
settings-update-channel = Channel
channel-stable = Stable
channel-beta = Beta
settings-save = Save
settings-revert = Revert
settings-saved = Settings saved.
settings-save-failed = Could not save settings: { $error }
hotkey-invalid = Invalid: { $error }
hotkey-duplicate = Already used by "{ $other }"
hotkey-conflict = Not registered: { $reason }

## Privacy
privacy-title = Privacy audit
privacy-network = Network: Hyper Clips only contacts GitHub to check for updates, and only if enabled.
privacy-folders = Folders used
privacy-mode = Mode: { $mode }
mode-installed = installed
mode-portable = portable

## Status bar
status-core-connected = Core v{ $version } · { $core_mem } · Studio { $ui_mem }
status-core-missing = Core service not running

## Settings — capture & audio
settings-codec = Video codec
settings-rate-control = Rate control
settings-monitor = Monitor
monitor-game = Monitor of the game
monitor-primary = Primary monitor
settings-screenshot-format = Screenshot format
settings-audio = Audio
settings-audio-system = Record system audio (game, music, chat)
settings-audio-mic = Record microphone
settings-volume = Volume (%)
settings-audio-hint = With both sources on, clips contain a mixed track plus separate tracks for editing.

## Settings — phase 2 completion
settings-capture-mode = Capture
mode-monitor = Whole monitor
mode-window = Game window only
mode-region = Region
settings-region = Region (x, y, width, height)
settings-show-cursor = Show mouse cursor
settings-sdr-white = HDR → SDR paper white (nits)
settings-buffer-storage = Replay buffer
storage-auto = Automatic (disk above 2 min)
storage-memory = RAM
storage-disk = Disk
settings-webcam = Webcam
settings-webcam-enabled = Show webcam in recordings
settings-webcam-device = Camera
settings-webcam-size = Size (% of height)
settings-position = Position
pos-top-left = Top left
pos-top-right = Top right
pos-bottom-left = Bottom left
pos-bottom-right = Bottom right
settings-separate-tracks = Separate tracks for game, voice chat and other sounds
settings-ptt = Push-to-talk key (empty = mic always on)
settings-noise-gate = Noise gate
settings-noise-gate-db = Threshold (dBFS)
settings-noise-suppression = Noise suppression (RNNoise, local)
settings-notifications = Notifications
settings-notify-enabled = Show notifications
settings-notify-in-game = Show over the game (never recorded)
settings-notify-sound = Play a sound

## Library
lib-search = Search title, game, notes, tags…
lib-all-games = All games
lib-all-tags = All tags
lib-all-kinds = Everything
kind-clip = Clips
kind-recording = Recordings
kind-screenshot = Screenshots
lib-favorites = Favourites only
lib-sort = Sort
sort-newest = Newest
sort-oldest = Oldest
sort-longest = Longest
sort-shortest = Shortest
sort-largest = Largest
sort-title = Title
lib-group = Group
group-none = None
group-game = By game
group-session = By session
lib-empty = No clips yet. Press { $key } in game to save one.
lib-summary = { $count } items · { $size }
lib-scanning = Scanning folders…
lib-title = Title
lib-rename = Rename
lib-notes = Notes
lib-tags = Tags (comma separated)
lib-markers = Markers
lib-open-folder = Show in Explorer
lib-copy = Copy to clipboard
lib-copied = Copied: paste it in Discord or any app.
lib-open = Open
lib-delete = Delete
lib-delete-confirm = Move to the Recycle Bin?
lib-yes = Yes
lib-no = No
lib-close = Close
lib-player-error = This file cannot be played (missing codec?).
lib-favorite = Favourite
settings-library = Library and storage
settings-quota = Storage limit (GB, 0 = unlimited)
settings-auto-delete = Move the oldest non-favourite clips to the Recycle Bin when over the limit
settings-extra-folders = Extra folders (one per line)
