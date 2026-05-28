export default {
  apps: [
    {
      name: "typescript-prebuilt",
      script: "dist/server.js",
      cwd: ".",
      env: {
        PORT: "3102"
      }
    }
  ]
};

