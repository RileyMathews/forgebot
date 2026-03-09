{
  description = "Forgebot — A daemon that bridges Forgejo webhooks to opencode";

  inputs = {
    nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";
    naersk = {
      url = "github:nix-community/naersk";
      inputs.nixpkgs.follows = "nixpkgs";
    };
    flake-utils = {
      url = "github:numtide/flake-utils";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      naersk,
      flake-utils,
      ...
    }:
    let
      supportedSystems = [
        "x86_64-linux"
        "aarch64-linux"
        "x86_64-darwin"
        "aarch64-darwin"
      ];

      # Build the package for a given system
      mkPackage = system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [];
          };

          naersk-lib = pkgs.callPackage naersk { };

          # Common build inputs for the Rust project
          nativeBuildInputs = with pkgs; [
            pkg-config
            rustc
            cargo
          ];

          buildInputs = with pkgs; [
            openssl
            sqlite
          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];
        in
        naersk-lib.buildPackage {
          pname = "forgebot";
          version = "0.1.0";
          root = ./.;

          inherit nativeBuildInputs buildInputs;

          # Copy SQLx query metadata for offline builds
          preBuild = ''
            export SQLX_OFFLINE=true
          '';

          meta = with pkgs.lib; {
            description = "A daemon that bridges Forgejo webhooks to opencode";
            homepage = "https://github.com/rileyforge/forgebot";
            license = licenses.mit;
            maintainers = [ ];
            platforms = platforms.unix;
            mainProgram = "forgebot";
          };
        };

      # Build devShell for a given system
      mkDevShell = system:
        let
          pkgs = import nixpkgs {
            inherit system;
            overlays = [];
          };
        in

        pkgs.mkShell {
          nativeBuildInputs = with pkgs; [
            pkg-config
          ];

          buildInputs = with pkgs; [
            # Rust toolchain
            rustc
            cargo
            rust-analyzer
            clippy
            rustfmt

            # SQLx CLI for running migrations
            sqlx-cli

            # Database and crypto dependencies
            openssl
            sqlite

            # Process Compose for running the app
            process-compose

          ] ++ pkgs.lib.optionals pkgs.stdenv.isDarwin [
            pkgs.darwin.apple_sdk.frameworks.Security
            pkgs.darwin.apple_sdk.frameworks.SystemConfiguration
          ];
          shellHook = ''
            echo "Forgebot development shell"
            echo "Rust version: $(rustc --version)"
            echo "Cargo version: $(cargo --version)"
            echo ""
            echo "Available commands:"
            echo "  cargo build         - Build the project"
            echo "  cargo test          - Run tests"
            echo "  cargo clippy        - Run linter"
            echo "  sqlx migrate        - Run database migrations"
            echo "  process-compose up  - Build and run the app"
            echo ""
          '';
        };
    in
    {
      # NixOS module
      nixosModules = {
        forgebot = import ./nix/module.nix self;
        default = self.nixosModules.forgebot;
      };

      # Home Manager module (optional, for user-level deployment)
      homeManagerModules = {
        forgebot = import ./nix/module.nix self;
        default = self.homeManagerModules.forgebot;
      };
    }
    // flake-utils.lib.eachSystem supportedSystems (system: {
      packages = {
        forgebot = mkPackage system;
        default = mkPackage system;
      };

      devShells = {
        default = mkDevShell system;
      };

      # Apps for `nix run`
      apps = {
        forgebot = {
          type = "app";
          program = "${mkPackage system}/bin/forgebot";
        };
        default = {
          type = "app";
          program = "${mkPackage system}/bin/forgebot";
        };
      };
    });
}
