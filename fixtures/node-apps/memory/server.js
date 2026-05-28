const chunks = [];
setInterval(() => {
  chunks.push(Buffer.alloc(1024 * 1024));
  console.log(`allocated ${chunks.length} MiB`);
}, 100);

