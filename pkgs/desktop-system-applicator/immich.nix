{ config, ... }:

let
  # TODO: replace with the real hostname, and with the tunnel UUID printed by
  # `cloudflared tunnel create photos`.
  domain = "photos.example.com";
  tunnelId = "00000000-0000-0000-0000-000000000000";
  credentialsFile = "/var/lib/cloudflared/photos.json";
in
{
  # Immich stays on loopback; the Cloudflare Tunnel is what makes it public.
  # The tunnel dials out, so it opens no ports and keeps serving while wgnord
  # holds the default route.
  services.immich = {
    enable = true;
    host = "127.0.0.1";
    port = 2283;
    mediaLocation = "/var/lib/immich";
    machine-learning.enable = true;
    accelerationDevices = [
      "/dev/dri/renderD128"
      "/dev/dri/renderD129"
    ];
    settings = {
      newVersionCheck.enabled = false;
      server.externalDomain = "https://${domain}";
    };
  };

  # credentialsFile is deployed out of band and read by systemd through
  # LoadCredential, so the secret never enters the Nix store.
  services.cloudflared = {
    enable = true;
    tunnels.${tunnelId} = {
      inherit credentialsFile;
      ingress.${domain} = "http://127.0.0.1:${toString config.services.immich.port}";
      default = "http_status:404";
    };
  };
}
