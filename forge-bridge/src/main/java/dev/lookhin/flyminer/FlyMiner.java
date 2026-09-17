package dev.lookhin.flyminer;

import net.minecraftforge.api.distmarker.Dist;
import net.minecraftforge.fml.DistExecutor;
import net.minecraftforge.fml.common.Mod;

@Mod(FlyMiner.ID)
public final class FlyMiner {
    public static final String ID = "flyminer";

    public FlyMiner() {
        DistExecutor.safeRunWhenOn(Dist.CLIENT, () -> ClientBridge::initialize);
    }
}
