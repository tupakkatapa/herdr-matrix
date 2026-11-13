# Downloads the models the bot needs to `dir`, each checked against its
# sha256, and restarts the bot after a new one: it looks for its models
# only as it starts
{ lib
, writeShellApplication
, coreutils
, curl
, systemd
, dir
, files
}:
writeShellApplication {
  name = "herdr-matrix-models";
  runtimeInputs = [ coreutils curl systemd ];
  text = ''
    mkdir -p ${dir}
    status=0
    fetched=0
    while read -r path url sha256; do
      [[ -e $path ]] && continue
      echo "downloading $url"
      if ! curl -fsSL --connect-timeout 30 --max-time 600 -C - -o "$path.part" "$url"; then
        echo "failed to download $url"
        status=1
      elif echo "$sha256  $path.part" | sha256sum -c --status; then
        mv "$path.part" "$path"
        fetched=1
      else
        echo "wrong hash: $url"
        rm "$path.part"
        status=1
      fi
    done <<'EOF'
    ${lib.concatMapStringsSep "\n" (file: "${file.path} ${file.url} ${file.sha256}") files}
    EOF
    if ((fetched)); then
      systemctl --user try-restart --no-block herdr-matrix.service
    fi
    exit "$status"
  '';
}
