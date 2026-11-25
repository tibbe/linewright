# Design system

The web UI follows [daisyUI](https://daisyui.com/components/), on
[Tailwind CSS](https://tailwindcss.com/docs), with two themes of its own, and
its tokens are the classes of daisyUI and Tailwind.

## Colour

- **Light or dark.** The light theme, or the dark theme for a reader who
  prefers dark. Each is daisyUI's built-in theme of that name, with the values
  of the table at the end of this section in place of its own.

| Variable | Light | Dark |
| --- | --- | --- |
| `--color-base-100` | `oklch(96% 0.008 68)` | `oklch(0.2046 0.007 257.8)` |
| `--color-base-200` | `oklch(97% 0.008 68)` | `oklch(0.2286 0.007 257.8)` |
| `--color-base-300` | `oklch(98% 0.008 68)` | `oklch(0.2486 0.007 257.8)` |
| `--color-base-content` | `oklch(21% 0.006 285.885)` | `oklch(97.807% 0.029 256.847)` |
| `--color-primary` | `oklch(46% 0.228 260.92)` | `oklch(46% 0.228 260.92)` |
| `--color-primary-content` | `oklch(93% 0.034 272.788)` | `oklch(96% 0.018 272.314)` |
| `--color-secondary` | `oklch(68% 0.163 184.32)` | `oklch(68% 0.163 184.32)` |
| `--color-secondary-content` | `oklch(98% 0.014 180.72)` | `oklch(94% 0.028 342.258)` |
| `--color-accent` | `oklch(0.3627 0.1049 260.92)` | `oklch(0.3627 0.1049 260.92)` |
| `--color-accent-content` | `oklch(0.7318 0.1049 260.92)` | `oklch(0.7318 0.1049 260.92)` |
| `--color-neutral` | `oklch(14% 0.052 265.87)` | `oklch(43% 0.046 261.32)` |
| `--color-neutral-content` | `oklch(92% 0.004 286.32)` | `oklch(92% 0.004 286.32)` |
| `--color-info` | `oklch(83% 0.094 211.43)` | `oklch(29% 0.066 243.157)` |
| `--color-info-content` | `oklch(29% 0.066 243.157)` | `oklch(83% 0.094 211.43)` |
| `--color-success` | `oklch(82% 0.092 163.21)` | `oklch(37% 0.077 168.94)` |
| `--color-success-content` | `oklch(37% 0.077 168.94)` | `oklch(82% 0.092 163.21)` |
| `--color-warning` | `oklch(83% 0.092 76.54)` | `oklch(41% 0.112 45.904)` |
| `--color-warning-content` | `oklch(41% 0.112 45.904)` | `oklch(83% 0.092 76.54)` |
| `--color-error` | `oklch(82% 0.112 24.87)` | `oklch(27% 0.105 12.094)` |
| `--color-error-content` | `oklch(27% 0.105 12.094)` | `oklch(82% 0.112 24.87)` |
| `--radius-selector` | `0.5rem` | `0.5rem` |
| `--radius-field` | `0.25rem` | `0.25rem` |
| `--radius-box` | `0.5rem` | `0.5rem` |
| `--size-selector` | `0.25rem` | `0.25rem` |
| `--size-field` | `0.25rem` | `0.25rem` |
| `--border` | `0.5px` | `0.5px` |
| `--depth` | `1` | `1` |
| `--noise` | `0` | `0` |

## Additional design tokens

Tokens that Linewright adds to those of daisyUI and Tailwind. The spec names
each by its ID, as it names theirs.

- `secondary-text`: text in `text-base-content/60`.

## Layout

- **Window size classes.** Compact, below Tailwind's `lg` width, and
  expanded, from `lg`.
- **Spacing.** The parts of a screen are `gap-6` apart.
- **Scrolling.** Content wider than the screen scrolls sideways on its own,
  not the screen with it.
- **Hover and focus.** What a part shows on hover, it also shows on keyboard
  focus. It shows over the screen until the pointer leaves the part or focus
  moves. Text cut to fit shows whole this way, in a daisyUI `tooltip`.

## Icons

- **The icon set** is [Heroicons](https://heroicons.com) 2, in its 20 solid
  ("mini") style. The exceptions are the icons of an issue and of a pull
  request, which are GitHub's own, from
  [Octicons](https://primer.style/octicons).
- **An item's icon** is Octicons' `issue-opened` for an issue, or
  `git-pull-request` for a pull request.
- **Alignment.** An icon in a line of text is centred vertically on that line.
- **A component's own icons.** Where a daisyUI component draws an icon of its
  own, such as the arrow of a `select`, it keeps it.
- **Sizes.** An icon in a daisyUI component is the size that daisyUI's
  examples of that component give it, in the table below.

| Component | Icon size      |
| --------- | -------------- |
| `alert`   | `size-6`       |
| `badge`   | `size-[1em]`   |
| `btn`     | `size-[1.2em]` |
| `input`   | `size-[1em]`   |
| `menu`    | `size-5`       |

## Status

- **Statuses.** A status is one of those in the table below, with its icon and
  its colour. Each area says which status each of its states shows as. A part
  that shows a status shows it by its icon, its colour or both, as the part
  says.

| Status    | Icon           | Colour            |
| --------- | -------------- | ----------------- |
| done      | `check-circle` | `success`         |
| failed    | `x-circle`     | `error`           |
| waiting   | `hand-raised`  | `warning`         |
| in flight | `arrow-path`   | `info`            |
| paused    | `pause-circle` | `base-content/60` |
| cancelled | `no-symbol`    | `neutral`         |

## Screens

- **Pages.** A screen that is not a dialog ([Dialog](components.md#dialog)) is
  a page, at the address that its section gives. Its link is its address.
- **Titles.** A screen's title is the page's `<title>`, which the browser shows
  on its tab. It is the name in the screen's heading, then "Linewright", such
  as "Benchmark run 14 · Linewright". For a screen of the navigation, the name
  is the screen's name in the navigation. For a screen that shows without the
  shell, the name is the screen's name in its area's table of screens, such as
  "Sign in · Linewright".
- **Live.** A screen shows each change to what it holds as the change
  happens, without the reader opening the screen again. What the reader opened
  or closed stays so through updates, until they open the screen again.

## Words

- **What is so.** A screen states what is so, not what to do about it; how to
  work with Linewright is the user guide's.
- **Voice.** A sentence on a screen is in the active voice: it names who or
  what acts, such as "This signs them out and ends their authorizations."
- **Buttons.** The label of a button that acts on something is a verb, or
  "New" for a button that makes something. The label also names what the
  button acts on where the screen does not show it, such as "Add task" where
  no task shows.

## Figures

- **Absent.** A figure that is absent, such as a mean with nothing to count,
  shows as "–".

## Times

- **Past times.** A past time is relative, in words, rounded down to whole
  minutes, hours or days, such as "5 minutes ago". It keeps up as time passes.
  Under a minute, it is "just now". Past 30 days, it is the date, such as "on
  Sep 12", with the year when it is not this year, such as "on Sep 12, 2025".
  On hover, it shows the absolute time, in the reader's time zone.
- **Future times.** A future time is absolute, in the reader's time zone.
- **Durations.** A duration is in narrow units, to its two largest units that
  are not zero, such as "45s", "1m 43s", "3h 2m" or "1d 5m". Under a second,
  it is "0s". For a run in flight, it keeps up as time passes.
- **Waiting.** Where a screen shows how long something else has gone on, it
  shows when it began, as a past time.

## Buttons

- **Icons.** A button whose label is a verb has an icon before its label that
  stands for what it does. A button in the table below has its icon wherever
  it is.

| Button                                                  | Icon            |
| ------------------------------------------------------- | --------------- |
| "Add THING"                                             | `plus`          |
| "Edit"                                                  | `pencil-square` |
| "Save"                                                  | `check`         |
| "Remove"                                                | `trash`         |
| "Cancel" that stops work                                | `stop-circle`   |
| The button that backs out of a dialog, such as "Cancel" | `x-mark`        |

## Actions

- **Asking first.** An action that stops work or removes what Linewright
  cannot bring back asks first, in a
  [confirmation](components.md#confirmation).
- **Changing.** A thing that the reader can change has "Edit", which opens an
  [edit dialog](components.md#edit-dialog). For a thing with a screen of its
  own, "Edit" is a [button](components.md#button) with the screen's buttons
  ([Shell](shell.md)). For any other thing, it is a
  [row button](components.md#row-button) at the end of its row.
- **What was made.** When a screen does not show what its dialog made, it
  shows a [success message](components.md#success-message) under its heading,
  which says what was made and links to it.
- **What was removed.** When the reader removes the thing that a screen shows,
  the screen of what held it opens, with a
  [success message](components.md#success-message) under its heading, which
  says what was removed.
