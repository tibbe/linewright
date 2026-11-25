# Design system

The web UI follows daisyUI, on Tailwind, with two themes of its own, and its
tokens are the classes of daisyUI and Tailwind.

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

## Type

- **Text** is in `base-content`.
- **Secondary text** ([Type](ui_patterns.md#type)) is in
  `text-base-content/60`.

## Layout

- **Window size classes.** Compact, below Tailwind's `lg` width, and
  expanded, from `lg`.

## Icons

- **The icon set** is [Heroicons](https://heroicons.com) 2, in its 20 solid
  ("mini") style. The exceptions are the icons of an issue and of a pull
  request, which are GitHub's own, from
  [Octicons](https://primer.style/octicons).
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
