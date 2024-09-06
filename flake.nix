{
  description = "Flea - a very experimental Rust GUI";

  inputs = {
    nixpkgs.url = "github:nixos/nixpkgs?ref=nixos-unstable";
    flake-utils.url = "github:numtide/flake-utils";
  };

  outputs = { self, nixpkgs, flake-utils }:
    let
      systems = with flake-utils.lib; [
        system."x86_64-linux"
        system."aarch64-linux"
        system."x86_64-darwin"
        system."aarch64-darwin"
      ];
    in
    flake-utils.lib.eachSystem systems (system:
      let
        pkgs = import nixpkgs {
          inherit system;
          config = {
            allowUnfree = true;
            allowUnfreePredicate = (_: true);
          };
        };

      in
      {
        devShells.default = pkgs.mkShell {
          buildInputs = with pkgs; [
            libiconv
            llvmPackages_latest.llvm
            llvmPackages_latest.bintools
            llvmPackages_latest.lld
            darwin.apple_sdk.frameworks.Security
            darwin.apple_sdk.frameworks.Carbon
            darwin.apple_sdk.frameworks.SystemConfiguration
            darwin.apple_sdk.frameworks.AppKit
            darwin.apple_sdk.frameworks.Foundation
            darwin.apple_sdk.frameworks.QuartzCore
            darwin.apple_sdk.frameworks.ApplicationServices
            rustc
            cargo
            pkg-config
            openssl
          ];
        };
      });
}
