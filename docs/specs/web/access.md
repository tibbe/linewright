# Access on the web

## Sign in

- **The page** has the card in its middle:
  `flex min-h-screen items-center justify-center p-4`.
- **The card** is a card ([Cards](design_system.md#cards)), with
  `w-full max-w-100 p-12`.
- **The heading** is in `text-3xl` and `font-bold`, centred, with `mb-2`
  under it. The repository's name is in secondary text, centred, with `mb-6`
  under it.
- **The message** is a daisyUI `alert`, with `mb-6` under it. It is in the
  `alert-error` style, but for "You signed out." and "Your sign-in ended.",
  which get the plain `alert`.
- **The identity providers' buttons** are one under the other, in
  `flex flex-col items-center gap-2`.
- **Google's button** is Google's own "Sign in with Google" button for the
  web, as Google gives it: its light one in the light theme and its dark one
  in the dark theme, at its own size.
- **GitHub's button** is a daisyUI `btn`, not `btn-sm`, with GitHub's mark
  before its label. The mark is GitHub's black one in the light theme and
  its white one in the dark theme.

## Consent

- **The page, the card and the heading** are as on Sign in.
- **The facts** are [facts](components.md#facts) in the screen's card, with
  `mb-6` under them.
- **The warning** is a daisyUI `alert` in the `alert-warning` style, with
  `mb-6` under it.
- **"Allow" and "Deny"** are in `grid grid-cols-2 gap-2`. "Allow" is a
  `btn-primary`, with the icon `check`. "Deny" has the icon `x-mark`.

## Authorization error

- **The page, the card and the heading** are as on Sign in.
- **The message** is a daisyUI `alert` in the `alert-error` style.

## People

- **The icon** of People, in the navigation and on its card, is `users`.
- **A row's buttons.** "Disable" has the icon `lock-closed`, and "Enable"
  the icon `lock-open`.

## Edit person

- **Role** is a `select`.

## Authorizations

- **The icon** of the table's card is `key`.
- **"End"** has the icon `link-slash`.
