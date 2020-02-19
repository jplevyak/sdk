{ pkgs }:

let
  repoRoot = ../../..;
  src = pkgs.lib.noNixFiles (pkgs.lib.gitOnlySource repoRoot ./.);
in
pkgs.napalm.buildPackage src {
  root = ./.;
  name = "dfinity-sdk-userlib-js";
  outputs = [ "out" "lib" ];
  # ci script now does everything CI should do. Bundle is needed because it's the output
  # of the nix derivation.
  npmCommands = [
    "npm install"
    "npm run ci"
    "npm run bundle"
  ];

  installPhase = ''
    mkdir -p $out $lib
    cp -r node_modules $lib
    cp -R dist $out
    cp package.json $out
    cp README.adoc $out
  '';
}
