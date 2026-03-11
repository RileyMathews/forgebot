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
    opencode = {
      url = "github:anomalyco/opencode?ref=v1.2.23";
    };
  };

  outputs =
    {
      self,
      nixpkgs,
      naersk,
      flake-utils,
      opencode,
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

            # Python for local automation scripts
            python3

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
    // flake-utils.lib.eachSystem supportedSystems (
      system:
      let
        pkgs = import nixpkgs {
          inherit system;
          overlays = [ ];
        };

        forgebot-pkg = mkPackage system;
        forgejo-mcp-pkg = pkgs.buildGoModule {
          pname = "forgejo-mcp";
          version = "2.15.0";
          src = pkgs.fetchzip {
            url = "https://codeberg.org/goern/forgejo-mcp/archive/v2.15.0.tar.gz";
            hash = "sha256-QFpGGmAl94vppOkMm+w4GQ1/bLtvkpWXG8JgBQJGvfw=";
          };
          vendorHash = "sha256-j5o/FZBowQvcatw14Fvs/8CTM5ZtQR6kwlroctaeKuM=";
          ldflags = [
            "-s"
            "-w"
            "-X"
            "main.Version=2.15.0"
          ];
          doCheck = false;

          meta = with pkgs.lib; {
            description = "MCP server for Forgejo";
            homepage = "https://codeberg.org/goern/forgejo-mcp";
            license = licenses.agpl3Only;
            maintainers = [ ];
            platforms = platforms.unix;
            mainProgram = "forgejo-mcp";
          };
        };
      in
      {
        packages = {
          forgebot = forgebot-pkg;
          forgejo-mcp = forgejo-mcp-pkg;
          default = forgebot-pkg;
        };

        devShells = {
          default = mkDevShell system;
        };

        # Apps for `nix run`
        apps = {
          forgebot = {
            type = "app";
            program = "${forgebot-pkg}/bin/forgebot";
          };
          default = {
            type = "app";
            program = "${forgebot-pkg}/bin/forgebot";
          };
        };
      }
    );
}
