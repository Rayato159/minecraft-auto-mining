package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.world.entity.LivingEntity;
import net.minecraft.world.inventory.ClickType;
import net.minecraft.world.item.SwordItem;

/** Prevent defensive sword swings from sweeping through nearby players or passive mobs. */
final class CombatSafety {
    static boolean safeSwordTick(Minecraft mc, LivingEntity target) {
        var nearby = mc.level.getEntitiesOfClass(LivingEntity.class, target.getBoundingBox().inflate(1.5,.75,1.5),
            e -> e != target && e != mc.player && e.isAlive() && !ClientBridge.hostileToPlayer(mc,e));
        if (!nearby.isEmpty()) {
            if (!BuiltInRegistries.ITEM.getKey(mc.player.getMainHandItem().getItem()).getNamespace().equals("minecraft")) {
                for (int i=0;i<36;i++) {
                    var stack=mc.player.getInventory().getItem(i);
                    if (stack.getItem() instanceof SwordItem && BuiltInRegistries.ITEM.getKey(stack.getItem()).getNamespace().equals("minecraft") && ClientBridge.matches(mc,stack,"sword")) {
                        int slot=i;
                        if(slot>=9){mc.gameMode.handleInventoryMouseClick(mc.player.inventoryMenu.containerId,slot,8,ClickType.SWAP,mc.player);slot=8;}
                        mc.player.getInventory().selected=slot;
                        return false;
                    }
                }
                throw new IllegalStateException("Cannot bound this sword's area attack around nearby friendly entities; separate from them first.");
            }
            // A falling critical strike disables vanilla sword sweeping.
            if(mc.player.onGround()) {
                if(!mc.level.noCollision(mc.player,mc.player.getBoundingBox().move(0,1.3,0)) ||
                    !ClientBridge.supportedDryBody(mc,mc.player.getBoundingBox()) ||
                    !MiningProfile.safeBody(mc,mc.player.getBoundingBox().move(0,1.3,0)))
                    throw new IllegalStateException("No safe critical-jump headroom near friendly entities; move clear before attacking.");
                mc.options.keyJump.setDown(true);
                return false;
            }
            mc.options.keyJump.setDown(false);
            if(mc.player.isInWater() || mc.player.isPassenger() || mc.player.onClimbable() ||
                mc.player.hasEffect(net.minecraft.world.effect.MobEffects.BLINDNESS) || mc.player.fallDistance<=.08F ||
                mc.player.getDeltaMovement().y>=-.04 || mc.player.getDeltaMovement().y<-.35) return false;
        }
        mc.player.setSprinting(false);
        return true;
    }
}
