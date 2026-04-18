# issue-new

Create a GitHub issue description for the user's report. Keep it:
simple, maintainer-friendly (avoid unnecessary technical jargon),
clear, concise, and free of duplicate information. Write it as if an
end user found the issue.

Title format: `[Type]: <Title>`

Types:

- Feature
- Bug
- Improvement
- Doc
- Question

Example: `[Feature] Retry failed makepkg runs with --clean`

## Body format

### Short summary

Describe the issue in at most two sentences.

### Body

Add this section only when additional detail is needed beyond the short
summary. Prefer a bulleted list.

Please include (when it's a bug):

- aur-pkgbuilder version or commit hash.
- Arch-based distribution + kernel.
- GTK / libadwaita versions (`pkg-config --modversion gtk4 libadwaita-1`).
- Whether it happens on Wayland or X11.
- Relevant external tool versions (`makepkg --version`, `ssh -V`).
- Steps to reproduce.
