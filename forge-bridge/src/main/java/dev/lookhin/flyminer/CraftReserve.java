package dev.lookhin.flyminer;

/** Group quotas. Keep whole stacks until the quota is met; deposit later stacks. */
final class CraftReserve {
    static int limit(String group, boolean needsPick) {
        return switch(group) {
            case "stone", "planks", "logs", "minecraft:stick", "fuel" -> 64;
            case "minecraft:diamond", "minecraft:iron_ingot", "raw_iron" -> needsPick ? 3 : 0;
            case "minecraft:crafting_table", "minecraft:furnace" -> needsPick ? 1 : 0;
            default -> 0;
        };
    }
    static boolean keep(String group, boolean needsPick, int earlier) {
        return earlier < limit(group, needsPick);
    }
}
