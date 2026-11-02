# Patch mirror

`0001-irix7-run-the-native-PAL-on-the-v0-syscall-gate.patch`,
`0002-irix7-format-the-native-PAL-changes.patch` and
`0003-irix7-add-native-file-calls-and-directory-cookies.patch` are
`git format-patch a36041ac..d5fd3d04` from the `irix7-pal` branch of
`https://github.com/irix7/relibc`.

They apply in order on top of
`a36041ac5201ede0c3853d548de5aa7fbfb0c708` and are kept only so the change can
be reviewed in this repository.  The branch is the source of truth;
`tools/relibc-pal/build.sh` pins and verifies the commit, not the patches.
