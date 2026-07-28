let checked = 0;

for (const file of Deno.args) {
  const packet = JSON.parse(await Deno.readTextFile(file));
  if (!Array.isArray(packet.references) || packet.references.length === 0) {
    throw new Error(`context packet has no references: ${file}`);
  }
  if (packet.used_chars > packet.max_chars) {
    throw new Error(`context packet exceeds its budget: ${file}`);
  }
  for (const reference of packet.references) {
    if (
      typeof reference.source_uri !== "string" ||
      !reference.source_uri.startsWith("legacy://")
    ) {
      continue;
    }
    const path = reference.source_uri.slice("legacy://".length);
    const source = await Deno.readFile(path);
    const exact = new TextDecoder().decode(
      source.slice(reference.start_byte, reference.end_byte),
    );
    if (exact !== reference.content) {
      throw new Error(`context citation does not match its source: ${file}`);
    }
    checked += 1;
  }
}

console.log(JSON.stringify({ exact_byte_citations_checked: checked }));
