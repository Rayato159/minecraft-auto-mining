package dev.lookhin.flyminer;

/** Pure resource policy shared by the live executor and offline tests. */
final class CraftPlan {
    enum Tier { DIAMOND, IRON, STONE, UNAVAILABLE }
    record Stock(int diamonds, int ingots, int rawIron, int stone, int planks, int sticks,
                 boolean table, boolean furnace, boolean coal,
                 boolean diamondHarvests, boolean ironHarvests, boolean stoneHarvests) {}

    static Tier select(Stock s) {
        int constructionWood = (s.table ? 0 : 4) + (s.sticks >= 2 ? 0 : 2);
        if (s.planks < constructionWood) return Tier.UNAVAILABLE;
        if (s.diamonds >= 3 && s.diamondHarvests) return Tier.DIAMOND;
        if (s.ingots >= 3 && s.ironHarvests) return Tier.IRON;
        boolean furnace = s.furnace || s.stone >= 8;
        boolean fuel = s.coal || s.planks - constructionWood >= 2;
        if (s.ingots + s.rawIron >= 3 && furnace && fuel && s.ironHarvests) return Tier.IRON;
        if (s.stone >= 3 && s.stoneHarvests) return Tier.STONE;
        return Tier.UNAVAILABLE;
    }
}
