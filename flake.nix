{
  description = "A fast, lightweight fetch written in Rust that is aware of your distro";

  inputs.nixpkgs.url = "github:NixOS/nixpkgs/nixos-unstable";

  outputs = { self, nixpkgs }:
    let
      systems = [ "x86_64-linux" ];
      forAllSystems = f: nixpkgs.lib.genAttrs systems (system: f nixpkgs.legacyPackages.${system});
    in
    {
      packages = forAllSystems (pkgs:
        let
          novafetch = pkgs.rustPlatform.buildRustPackage {
            pname = "novafetch";
            version = "1.1.0";
            src = ./.;
            cargoLock.lockFile = ./Cargo.lock;

            nativeBuildInputs = [ pkgs.makeWrapper ];

            postInstall = ''
              wrapProgram $out/bin/novafetch \
                --prefix PATH : ${pkgs.lib.makeBinPath [ pkgs.pciutils ]}
            '';

            meta = with pkgs.lib; {
              description = "A fast, lightweight fetch written in Rust that is aware of your distro";
              homepage = "https://github.com/JerrySM64/novafetch";
              license = licenses.mit;
              mainProgram = "novafetch";
              platforms = platforms.linux;
            };
          };
        in
        {
          inherit novafetch;
          default = novafetch;
        });

      devShells = forAllSystems (pkgs: {
        default = pkgs.mkShell {
          buildInputs = with pkgs; [
            cargo
            rustc
            rustfmt
            clippy
            pciutils
          ];
        };
      });
    };
}
