# sd completions

Shell completion scripts.

```bash
sd completions bash > ~/.local/share/bash-completion/completions/sd
sd completions zsh -o ~/.zfunc          # writes ~/.zfunc/_sd
sd completions fish -o ~/.config/fish/completions
```

- Shells: `bash`, `zsh`, `fish`, `powershell`, `elvish`, as br.
- The script is generated from `sd`'s own command definition, so it lists
  every verb and flag of the binary that printed it and cannot drift.
- Needs no configuration and never creates a ledger.
- `-o <dir>` writes the shell's conventional file name there (`sd.bash`,
  `_sd`, `sd.fish`, `_sd.ps1`, `sd.elv`).
