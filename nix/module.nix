self:
{ config, lib, pkgs, ... }:

let
  cfg = config.services.forgebot;
in
{
  options.services.forgebot = {
    enable = lib.mkEnableOption "forgebot daemon — a webhook bridge between Forgejo and opencode";

    package = lib.mkOption {
      type = lib.types.package;
      default = self.packages.${pkgs.stdenv.hostPlatform.system}.forgebot;
      defaultText = lib.literalExpression "self.packages.\${pkgs.stdenv.hostPlatform.system}.forgebot";
      description = ''
        The forgebot package to use.

        Defaults to the package from the forgebot flake.
      '';
    };

    dataDir = lib.mkOption {
      type = lib.types.path;
      default = "/var/lib/forgebot";
      description = "Directory where forgebot stores its data (database, worktrees, opencode config).";
    };

    user = lib.mkOption {
      type = lib.types.str;
      default = "forgebot";
      description = "User account under which forgebot runs.";
    };

    group = lib.mkOption {
      type = lib.types.str;
      default = "forgebot";
      description = "Group under which forgebot runs.";
    };

    opencodePackage = lib.mkOption {
      type = lib.types.nullOr lib.types.package;
      default = pkgs.opencode or null;
      defaultText = lib.literalExpression "pkgs.opencode or null";
      description = "The opencode package to make available in the service PATH. If null, opencode must be available in the system PATH or configured with an absolute path in forgebot.toml.";
    };

    secretsFilePath = lib.mkOption {
      type = lib.types.nullOr lib.types.path;
      default = null;
      example = lib.literalExpression "/run/secrets/forgebot";
      description = ''
        Path to a file containing secret environment variables.
        This file is loaded via systemd's EnvironmentFile directive.

        The file should contain lines like:
          FORGEBOT_WEBHOOK_SECRET=your-webhook-secret
          FORGEBOT_FORGEJO_TOKEN=your-api-token

        This enables integration with sops-nix or other secret management systems.
        Note: FORGEBOT_FORGEJO_URL is set via the forgejo.url option, not in secrets.
      '';
    };

    environment = lib.mkOption {
      type = lib.types.attrsOf lib.types.str;
      default = { };
      example = lib.literalExpression ''
        {
          RUST_LOG = "debug";
          RUST_BACKTRACE = "1";
        }
      '';
      description = "Additional environment variables for the forgebot service.";
    };

    # =============================================================================
    # Server configuration
    # =============================================================================
    server = lib.mkOption {
      type = lib.types.submodule {
        options = {
          host = lib.mkOption {
            type = lib.types.str;
            default = "127.0.0.1";
            example = "0.0.0.0";
            description = ''
              The host address to bind the HTTP server to.
              Use "127.0.0.1" for localhost-only (with reverse proxy),
              or "0.0.0.0" to listen on all interfaces.
            '';
          };

          port = lib.mkOption {
            type = lib.types.port;
            default = 8765;
            example = 8080;
            description = ''
              The TCP port to listen on.
              Ports below 1024 require root privileges. Use 8080 or higher,
              or configure a reverse proxy for HTTPS.
            '';
          };

          forgeBotHost = lib.mkOption {
            type = lib.types.nullOr lib.types.str;
            default = null;
            example = "https://forgebot.example.com";
            description = ''
              The public-facing URL where forgebot is accessible from the internet.
              This is used for webhook URLs displayed in the setup UI and for 
              registering webhooks with Forgejo.
              
              If not set, defaults to http://<server.host>:<server.port>.
              For production deployments behind a reverse proxy, set this to
              your public HTTPS URL (e.g., https://forgebot.example.com).
            '';
          };
        };
      };
      default = { };
      description = "HTTP server configuration for the webhook receiver and setup UI.";
    };

    # =============================================================================
    # Forgejo integration
    # =============================================================================
    forgejo = lib.mkOption {
      type = lib.types.submodule {
        options = {
          url = lib.mkOption {
            type = lib.types.str;
            example = "https://git.example.com";
            description = ''
              Base URL of your Forgejo instance.
              This is the non-secret URL used to connect to the Forgejo API.
              Example: https://git.example.com or https://code.example.com
            '';
          };

          botUsername = lib.mkOption {
            type = lib.types.str;
            default = "forgebot";
            example = "forgebot";
            description = ''
              The username that forgebot will operate as.
              Used to identify bot comments, set git commit author, and filter self-triggered webhooks.
              The token must belong to this user.
            '';
          };
        };
      };
      default = { };
      description = "Forgejo integration settings for API access and webhook handling.";
    };

    # =============================================================================
    # opencode integration
    # =============================================================================
    opencode = lib.mkOption {
      type = lib.types.submodule {
        options = {
          binary = lib.mkOption {
            type = lib.types.str;
            default = "opencode";
            example = "/run/current-system/sw/bin/opencode";
            description = ''
              Path to the opencode binary.
              Use "opencode" to search in PATH, or an absolute path if opencode
              is not in the standard system PATH.
            '';
          };

          worktreeBase = lib.mkOption {
            type = lib.types.path;
            default = "/var/lib/forgebot/worktrees";
            example = "/var/lib/forgebot/worktrees";
            description = ''
              Base directory for git worktrees.
              forgebot creates a worktree for each issue inside this directory.
              Each worktree is an isolated checkout to prevent concurrent sessions
              from interfering with each other.
              
              Directory structure: <worktree_base>/<owner>_<repo>/<issue_number>/
            '';
          };

          configDir = lib.mkOption {
            type = lib.types.path;
            default = "/var/lib/forgebot/opencode-config";
            example = "/var/lib/forgebot/opencode-config";
            description = ''
              Directory for opencode configuration files.
              forgebot populates this with package.json, agent definitions, and tools.
              Do not modify these files manually — they are managed by forgebot.
            '';
          };
        };
      };
      default = { };
      description = "opencode agent integration settings.";
    };

    # =============================================================================
    # Database configuration
    # =============================================================================
    database = lib.mkOption {
      type = lib.types.submodule {
        options = {
          path = lib.mkOption {
            type = lib.types.path;
            default = "/var/lib/forgebot/forgebot.db";
            example = "/var/lib/forgebot/forgebot.db";
            description = ''
              Path to the SQLite database file.
              The database is created automatically on first run if it doesn't exist.
              Migrations run automatically on startup.
            '';
          };
        };
      };
      default = { };
      description = "SQLite database settings for persisting repository registrations and session state.";
    };
  };

  config = lib.mkIf cfg.enable (
    let
      # Build the service PATH
      servicePath = lib.makeBinPath (
        [ cfg.package pkgs.git ]
        ++ lib.optional (cfg.opencodePackage != null) cfg.opencodePackage
      );
    in
    {
      # Create the forgebot user
      users.users.${cfg.user} = {
        description = "Forgebot daemon user";
        isSystemUser = true;
        group = cfg.group;
        home = cfg.dataDir;
        createHome = true;
      };

      # Create the forgebot group
      users.groups.${cfg.group} = { };

      # Ensure data directory exists with correct permissions
      systemd.tmpfiles.rules = [
        "d '${cfg.dataDir}' 0755 ${cfg.user} ${cfg.group} -"
        "d '${cfg.opencode.worktreeBase}' 0755 ${cfg.user} ${cfg.group} -"
        "d '${cfg.opencode.configDir}' 0755 ${cfg.user} ${cfg.group} -"
      ];

      # Define the systemd service
      systemd.services.forgebot = {
        description = "Forgebot — Forgejo webhook bridge to opencode";
        wantedBy = [ "multi-user.target" ];
        after = [ "network-online.target" ];
        wants = [ "network-online.target" ];

        serviceConfig = {
          Type = "simple";

          # User and group
          User = cfg.user;
          Group = cfg.group;

          # Working directory (for relative paths in config)
          WorkingDirectory = cfg.dataDir;

          # State directory handling
          StateDirectory = "forgebot";
          StateDirectoryMode = "0755";
          CacheDirectory = "forgebot";
          CacheDirectoryMode = "0755";

          # Security hardening
          NoNewPrivileges = true;
          ProtectSystem = "strict";
          ProtectHome = true;
          PrivateTmp = true;
          PrivateDevices = true;
          ProtectKernelTunables = true;
          ProtectKernelModules = true;
          ProtectControlGroups = true;
          RestrictSUIDSGID = true;
          RestrictRealtime = true;
          RestrictNamespaces = true;
          LockPersonality = true;
          MemoryDenyWriteExecute = true;

          # Allow writing to data directory
          ReadWritePaths = [ cfg.dataDir ];

          # Main service command - no arguments needed, env vars configure everything
          ExecStart = "${cfg.package}/bin/forgebot";

          # Restart policy
          Restart = "on-failure";
          RestartSec = 10;
          StartLimitInterval = 60;
          StartLimitBurst = 3;

          # Environment variables - non-secret values from NixOS configuration
          Environment = [
            "PATH=${servicePath}:/run/current-system/sw/bin:/usr/bin:/bin"
            "RUST_LOG=info"
            "FORGEBOT_DATA_DIR=${cfg.dataDir}"
            # Non-secret forgebot configuration
            "FORGEBOT_SERVER_HOST=${cfg.server.host}"
            "FORGEBOT_SERVER_PORT=${toString cfg.server.port}"
            "FORGEBOT_FORGEJO_URL=${cfg.forgejo.url}"
            "FORGEBOT_FORGEJO_BOT_USERNAME=${cfg.forgejo.botUsername}"
            "FORGEBOT_OPENCODE_BINARY=${cfg.opencode.binary}"
            "FORGEBOT_OPENCODE_WORKTREE_BASE=${cfg.opencode.worktreeBase}"
            "FORGEBOT_OPENCODE_CONFIG_DIR=${cfg.opencode.configDir}"
            "FORGEBOT_DATABASE_PATH=${cfg.database.path}"
          ] 
          ++ lib.optional (cfg.server.forgeBotHost != null) "FORGEBOT_FORGEBOT_HOST=${cfg.server.forgeBotHost}"
          ++ lib.mapAttrsToList (name: value: "${name}=${value}") cfg.environment;

          # Load secrets from file if provided
          EnvironmentFile = lib.optional (cfg.secretsFilePath != null) cfg.secretsFilePath;

          # Graceful shutdown
          TimeoutStopSec = 30;
          KillSignal = "SIGTERM";
        };
      };
    }
  );
}
