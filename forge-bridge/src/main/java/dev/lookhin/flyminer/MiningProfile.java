package dev.lookhin.flyminer;

import com.google.gson.Gson;
import com.google.gson.JsonObject;
import net.minecraft.client.Minecraft;
import net.minecraft.core.BlockPos;
import net.minecraft.world.phys.AABB;
import net.minecraftforge.fml.loading.FMLPaths;
import java.io.IOException;
import java.nio.file.Files;
import java.nio.charset.StandardCharsets;
import java.util.Map;

/** Explicit player constraints. Reads local preferences; never changes Origins powers. */
final class MiningProfile {
    private static boolean dwarf;
    static void load() throws IOException {
        var file=FMLPaths.CONFIGDIR.get().resolve("flyminer/mining-profile.json");
        if(!Files.exists(file)) {
            Files.writeString(file,"{\"profile\":\"standard\"}\n",StandardCharsets.UTF_8);
            return;
        }
        if(Files.size(file)>4096)throw new IOException("Mining profile is too large.");
        try {
            JsonObject config=new Gson().fromJson(Files.readString(file,StandardCharsets.UTF_8),JsonObject.class);
            String profile=config.get("profile").getAsString();
            if(!profile.equals("standard")&&!profile.equals("dwarf"))throw new IllegalArgumentException("Unknown mining profile");
            dwarf=profile.equals("dwarf");
        } catch(RuntimeException error) {throw new IOException("Invalid flyminer/mining-profile.json",error);}
    }
    static boolean dwarf(){return dwarf;}
    static void requireCover(Minecraft mc){
        if(!safeBody(mc,mc.player.getBoundingBox()))
            throw new IllegalStateException("Dwarf profile requires a covered route. Move under a roof or into a tunnel.");
    }
    static boolean requiresCover(Minecraft mc){return dwarf && mc.level.dimensionType().hasSkyLight();}
    // Require permanent cover even at night: a long return trip can outlast the night.
    // Artificial torch light is deliberately irrelevant to this check.
    static boolean exposed(Minecraft mc,BlockPos pos){
        return requiresCover(mc) && (!mc.level.hasChunkAt(pos)||mc.level.canSeeSky(pos));
    }
    static boolean safeBody(Minecraft mc,AABB box){
        if(!requiresCover(mc))return true;
        return coveredBody(box,p->exposed(mc,p));
    }
    static boolean coveredBody(AABB box,java.util.function.Predicate<BlockPos> exposed){
        for(BlockPos p:BlockPos.betweenClosed(BlockPos.containing(box.minX+.00001,box.minY+.00001,box.minZ+.00001),
                BlockPos.containing(box.maxX-.00001,box.maxY-.00001,box.maxZ-.00001)))
            if(exposed.test(p))return false;
        return true;
    }
    static boolean mayMine(Minecraft mc,BlockPos pos){
        // Removing a surface/roof block must never open direct sky over the route.
        return !requiresCover(mc)||!exposed(mc,pos.above());
    }
    static boolean maySwim(boolean inWater,boolean dwarfProfile){return inWater&&!dwarfProfile;}
    static boolean supportedEscape(boolean swimming,boolean sameColumn,boolean floorSafe){
        return swimming || !sameColumn&&floorSafe;
    }
    static long paletteKey(int blockId,boolean torchSupport,boolean blockedOre,boolean exposed,boolean roofCut){
        return ((long)blockId<<4)|(torchSupport?1:0)|(blockedOre?2:0)|(exposed?4:0)|(roofCut?8:0);
    }
    static Map<String,Object> state(Minecraft mc){
        return Map.of("name",dwarf?"dwarf":"standard","canSwim",!dwarf,
            "requiresCover",requiresCover(mc),"underCover",safeBody(mc,mc.player.getBoundingBox()));
    }
}
