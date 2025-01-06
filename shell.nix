{ pkgs ? import <nixpkgs> {} }:

let 
  nix_lib = builtins.getFlake "git+https://github.com/zmrocze/nix-lib";
in
pkgs.mkShell {
  packages = with pkgs; [ 
  	pkg-config
  	openssl
  	# nix_lib.devShells.${builtins.currentSystem}.default
  ];
}
