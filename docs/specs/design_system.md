# Design system

## Liveness

- A screen shows `serve`'s state as it changes, without the reader opening
  it again.
- What the reader opened or closed stays so through updates, until they
  open the screen again.

## Words

- **What is so.** A screen states what is so, not what to do about it; how to
  work with Linewright is the user guide's.
- **Voice.** A sentence on a screen is in the active voice: it names who or
  what acts, such as "This stops its running replays and judges."
- **Buttons.** A button's label is a verb, or "New" for one that makes
  something, with what it acts on where its
  place on the screen does not show that, such as "Remove" on a case's row or
  "Cancel the experiment".

## Screen states

- **Empty.** A list with nothing in it shows "No" and what it lists in its
  place, such as "No runs" or "No suites", unless its screen shows it only
  while it holds something.
- **Lost.** A screen that loses `serve` keeps what it last showed, shows
  "Lost `serve` 5 minutes ago; reconnecting.", with the time as a past time
  ([Time](#time)), and reconnects on its own.
- **Nothing there.** A link can name an item, run, suite or experiment that
  does not exist. The screen it opens shows "No such" and the name, as the
  screens give it, such as "No such run 43/rework-9" or "No such item #812".
- **Loading.** A screen shows once it has all that it holds; it has no
  loading state.

## Filters

- **Where.** A screen's filters sit together above what they narrow, each
  labelled with what it filters on.
- **Applying.** A filter applies as soon as it is chosen from a list, and
  one typed in when the reader presses Enter.
- **Unset.** An unset filter narrows nothing: a list reads "All" and what it
  lists, such as "All stages", and a field to type in is empty.
- **Narrowing.** What the filters leave out is gone from the screen and from
  its counts, but for a count the screen says holds however it is filtered.
- **Links.** A screen's link carries its filters: a reader who opens the
  screen again, or follows its link, gets the same filters. A link to the
  screen from elsewhere, such as from the navigation, carries none unless it
  says.
- **Kept.** An update never changes a filter.
- **Opened and closed.** Changing a filter leaves what the reader opened or
  closed as it was.

## Time

- **Past times.** A past time is shown relative, in words and rounded down
  to whole minutes, hours or days, such as "5 minutes ago", keeping up as
  time passes, with the absolute time in the reader's time zone shown on
  hover. Under a minute it is "just now", and past 30 days it is the date,
  such as "on Sep 12", with the year when it is not this year, such as "on
  Sep 12, 2025".
- **Future times.** A future time is shown absolute, in the reader's time
  zone.
- **Durations.** A duration is shown in narrow units, to its two largest
  units that are not zero, such as "45s", "1m 43s", "3h 2m" or "1d 5m", and
  "0s" under a second, keeping up as time passes for a run in flight.
- **Waiting.** How long anything else has gone on is shown as when it
  began, a past time.

## Status

- **Statuses.** A status is one of done, failed, waiting, in flight, paused
  and cancelled. Each area says which status each of its states shows as.
- **Shown.** A status shows as its icon and its colour, never by colour
  alone.

## Tables

- As the reader scrolls, a table's column heads stay at the top.
- A title in a table is on one line.

## Facts

- A screen's facts about one thing are a list of labels, each with its value
  beside it.
- A label is words alone, with no icon or colour. A status among the facts
  is in the value: its icon and its words in its colour, with nothing behind
  them.

## Confirming

- **What asks first.** An action that stops work or removes what Linewright
  cannot bring back asks first, in a confirmation: a question that names
  what it acts on and says what follows.
- **Where.** A confirmation shows over the screen it was asked from, and has
  no link of its own.
- **The answer.** The reader confirms, and the action goes ahead, or turns
  it down, and nothing changes.
- **Its buttons.** The button that goes ahead has the label of the button
  that asked. The other says "Cancel", or "Keep running" where the action
  cancels.

## Layout

- The UI is laid out for someone scanning a screen.
- Content wider than the screen scrolls sideways on its own, not the screen
  with it.
- What a screen shows on hover it also shows on keyboard focus, over the
  screen, until the pointer leaves it or focus moves. Text cut to fit is shown
  whole this way.
- Only a name's title is cut to fit; its number and its link to GitHub
  always show.

## Type

- Secondary text, text that qualifies the text it sits beside, such as a
  card's line under its title, a fact's label or a transcript entry's
  time or duration, is quieter than it.

## Colour scheme

- Light or dark follows the reader's preference.
