# Security policy

Diorama loads wallpapers made by other people: packed files, textures, models, shaders, scripts and web pages. All of it is treated as untrusted input. If you find a way for a wallpaper to do any of the following, that is a security bug:

- crash Diorama in a way you can control;
- read or write files outside what it is allowed to;
- reach the network without permission;
- run code outside the script sandbox.

Bugs in the installer or updater count too.

## Reporting

Please report privately through GitHub: **Security → Report a vulnerability** on this repository. Do not open a public issue.

Include:

- what an attacker can do;
- the steps or a minimal file that triggers it. Make the file yourself; please don't send other people's Workshop content.

You will get an answer within a week. A fix is released as soon as it is ready, and you are credited unless you prefer not to be.

## Supported versions

Only the latest release receives fixes.
