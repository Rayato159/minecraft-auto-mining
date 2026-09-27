package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.tags.FluidTags;
import net.minecraft.world.entity.EquipmentSlot;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.Level;
import net.minecraft.world.phys.Vec3;
import net.minecraftforge.fml.ModList;
import java.lang.reflect.Method;
import java.util.LinkedHashMap;
import java.util.Map;

/** Read-only adapters for the installed mods. Never manufactures air or edits item NBT. */
final class AirSupply {
    record Quality(boolean known, boolean breathe, boolean refill, String name) {}
    private static boolean initialized;
    private static Object thinAir;
    private static Method qualityAt, tankAir, tankMax;
    private static String adapterError = "";
    private static String session = "";
    private static boolean escaping;
    private static net.minecraft.core.BlockPos lastFresh;

    private static void initialize() {
        if (initialized) return;
        initialized = true;
        try {
            if (ModList.get().isLoaded("thinair")) {
                Class<?> api = Class.forName("fuzs.thinair.api.v1.AirQualityHelper");
                thinAir = api.getField("INSTANCE").get(null);
                qualityAt = api.getMethod("getAirQualityAtLocation", Level.class, Vec3.class);
            }
        } catch (ReflectiveOperationException | LinkageError error) { adapterError = "ThinAir API unavailable"; }
        try {
            if (ModList.get().isLoaded("create")) {
                Class<?> api = Class.forName("com.simibubi.create.content.equipment.armor.BacktankUtil");
                tankAir = api.getMethod("getAir", ItemStack.class);
                tankMax = api.getMethod("maxAir", ItemStack.class);
            }
        } catch (ReflectiveOperationException | LinkageError error) { adapterError += " Create air API unavailable"; }
    }

    static Quality quality(Minecraft mc, Vec3 eyes) {
        initialize();
        if (!mc.level.hasChunkAt(net.minecraft.core.BlockPos.containing(eyes))) return new Quality(false,false,false,"unloaded");
        if (!ModList.get().isLoaded("thinair")) return new Quality(true,true,true,"vanilla");
        if (qualityAt == null) return new Quality(false,false,false,"unknown");
        try {
            Object q = qualityAt.invoke(thinAir, mc.level, eyes);
            // Enum constants have anonymous subclasses; these fields belong to the public enum.
            Class<?> type = Class.forName("fuzs.thinair.api.v1.AirQualityLevel");
            return new Quality(true, type.getField("canBreathe").getBoolean(q),
                type.getField("canRefillAir").getBoolean(q), ((Enum<?>)q).name());
        } catch (ReflectiveOperationException | RuntimeException error) { return new Quality(false,false,false,"unknown"); }
    }

    static boolean fresh(Minecraft mc, Vec3 eyes) {
        var p = net.minecraft.core.BlockPos.containing(eyes);
        if (!mc.level.hasChunkAt(p) || !mc.level.getFluidState(p).isEmpty() ||
                !mc.level.getBlockState(p).getCollisionShape(mc.level,p).isEmpty()) return false;
        Quality q = quality(mc,eyes);
        return q.known && q.refill;
    }

    static Map<String,Object> state(Minecraft mc) {
        initialize();
        String current = ClientBridge.sessionId(mc)+"|"+mc.player.getUUID()+"|"+mc.level.dimension().location();
        if (!current.equals(session)) { session=current; escaping=false; lastFresh=null; }
        var chest = mc.player.getItemBySlot(EquipmentSlot.CHEST);
        var helmet = mc.player.getItemBySlot(EquipmentSlot.HEAD);
        String chestId = net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(chest.getItem()).toString();
        String helmetId = net.minecraft.core.registries.BuiltInRegistries.ITEM.getKey(helmet.getItem()).toString();
        boolean gear = (chestId.equals("create:copper_backtank") || chestId.equals("create:netherite_backtank")) &&
            (helmetId.equals("create:copper_diving_helmet") || helmetId.equals("create:netherite_diving_helmet"));
        double remaining=0, capacity=0;
        if (gear && tankAir!=null && tankMax!=null) try {
            remaining=((Number)tankAir.invoke(null,chest)).doubleValue();
            capacity=((Number)tankMax.invoke(null,chest)).doubleValue();
        } catch (ReflectiveOperationException | RuntimeException error) { gear=false; }
        Quality q=quality(mc,mc.player.getEyePosition());
        boolean submerged=mc.player.isEyeInFluid(FluidTags.WATER);
        boolean natural=fresh(mc,mc.player.getEyePosition());
        if(natural)lastFresh=mc.player.blockPosition();
        int air=mc.player.getAirSupply(), max=Math.max(1,mc.player.getMaxAirSupply());
        // ThinAir/CreateAir consume about one tank unit per second in this pack.
        // Keep a generous reserve; the ordinary bubble bar can remain full until the tank empties.
        boolean exposed=submerged || !q.breathe || air<max-20;
        double escapeReserve=lastFresh==null?120:Math.max(120,45+Math.abs(mc.player.getY()-lastFresh.getY())*3+Math.hypot(mc.player.getX()-lastFresh.getX(),mc.player.getZ()-lastFresh.getZ())*.6);
        boolean need=low(air,max,exposed,submerged,gear,remaining,capacity,escapeReserve);
        if ((need || submerged) && !natural) escaping=true;
        if (natural && air>=max*.95) escaping=false;
        var result=new LinkedHashMap<String,Object>();
        result.put("known",q.known); result.put("quality",q.name); result.put("breathable",natural);
        result.put("air",air); result.put("maxAir",max); result.put("submerged",submerged);
        result.put("tankAir",remaining); result.put("tankCapacity",capacity); result.put("divingGear",gear);
        result.put("needsEscape",escaping); result.put("adapterError",adapterError);
        result.put("escapeReserveSeconds",escapeReserve);
        return result;
    }
    static boolean urgent(Minecraft mc) { return Boolean.TRUE.equals(state(mc).get("needsEscape")); }
    static boolean low(int air,int max,boolean exposed,boolean submerged,boolean gear,double tank,double capacity,double reserve){
        return air<=Math.max(1,max)*.65 || exposed&&(gear?tank<=Math.max(reserve,capacity*.15):!submerged||air<max*.9);
    }
    static void requireEntry(Minecraft mc,net.minecraft.core.BlockPos feet){
        var q=quality(mc,new Vec3(feet.getX()+.5,feet.getY()+mc.player.getEyeHeight(),feet.getZ()+.5));
        var s=state(mc);
        if(!q.known || !q.breathe&&(!Boolean.TRUE.equals(s.get("divingGear"))||((Number)s.get("tankAir")).doubleValue()<((Number)s.get("escapeReserveSeconds")).doubleValue()+60))
            throw new IllegalStateException("Destination air is unsafe or unknown; need charged breathing equipment and an exit reserve.");
    }

}
