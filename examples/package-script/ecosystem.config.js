export default {
  apps: [
    {
      name: "package-script",
      package_script: "start",
      package_manager: "npm",
      cwd: ".",
      env: {
        PORT: "3103"
      }
    }
  ]
};

