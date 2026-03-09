{ config, lib, pkgs, ... }:

let
  cfg = config.services.forgebot;

  # Default package from the flake
  defaultPackage = pkgs.forgebot or (throw "forgebot package not found in pkgs. Make sure the flake overlay is applied or set services.forgebot.package explicitly.");
in
{
  options.services.forgebot = {
    enable = lib.mkEnableOption "forgebot daemon — a webhook bridge between Forgejo and opencode";

    package = lib.mkOption {
      type = lib.types.package;
      default = defaultPackage;
      defaultText = lib.literalExpression "pkgs.forgebot";
      description = "The forgebot package to use.";
    };

    configFile = lib.mkOption {
      type = lib.types.path;
      example = lib.literalExpression "/etc/forgebot/forgebot.toml";
      description = "Path to the forgebot.toml configuration file.";
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
  };

  config = lib.mkIf cfg.enable (
    let
      # Build the service PATH
      servicePath = lib.makeBinPath (
        [ cfg.package ]
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
        "d '${cfg.dataDir}/worktrees' 0755 ${cfg.user} ${cfg.group} -"
        "d '${cfg.dataDir}/opencode-config' 0755 ${cfg.user} ${cfg.group} -"
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

          # Read configuration file
          ExecStartPre = [
            # Verify config file exists and is readable
            "${pkgs.coreutils}/bin/test -r ${lib.escapeShellArg cfg.configFile}"
          ];

          # Main service command
          ExecStart = "${cfg.package}/bin/forgebot --config ${lib.escapeShellArg cfg.configFile}";

          # Restart policy
          Restart = "on-failure";
          RestartSec = 10;
          StartLimitInterval = 60;
          StartLimitBurst = 3;

          # Environment variables
          Environment = [
            "PATH=${servicePath}:/run/current-system/sw/bin:/usr/bin:/bin"
            "RUST_LOG=info"
            "FORGEBOT_DATA_DIR=${cfg.dataDir}"
          ] ++ lib.mapAttrsToList (name: value: "${name}=${value}") cfg.environment;

          # Graceful shutdown
          TimeoutStopSec = 30;
          KillSignal = "SIGTERM";
        };
      };
    }
  );
}
