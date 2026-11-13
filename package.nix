{ rustPlatform
, lib
}:
let
  manifest = (lib.importTOML ./Cargo.toml).package;
in
rustPlatform.buildRustPackage {
  pname = manifest.name;
  inherit (manifest) version;

  src = lib.sourceByRegex ./. [
    "^Cargo.toml$"
    "^Cargo.lock$"
    "^src.*$"
    "^tests.*$"
  ];

  cargoLock.lockFile = ./Cargo.lock;

  meta = {
    inherit (manifest) description;
    homepage = "https://github.com/tupakkatapa/herdr-matrix";
    license = lib.licenses.gpl3Plus;
    mainProgram = manifest.name;
    platforms = lib.platforms.linux;
  };
}
