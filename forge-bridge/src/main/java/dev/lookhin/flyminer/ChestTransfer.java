package dev.lookhin.flyminer;

import net.minecraft.client.Minecraft;
import net.minecraft.client.gui.screens.inventory.AbstractContainerScreen;
import net.minecraft.core.BlockPos;
import net.minecraft.world.InteractionHand;
import net.minecraft.world.inventory.ChestMenu;
import net.minecraft.world.inventory.ClickType;
import net.minecraft.world.item.ItemStack;
import net.minecraft.world.level.ClipContext;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.ChestBlock;
import net.minecraft.world.level.block.TrappedChestBlock;
import net.minecraft.world.phys.HitResult;
import net.minecraft.world.phys.Vec3;

/** Sends ordinary server-validated menu clicks without local inventory prediction. */
final class ChestTransfer {
    private static BlockPos chest;
    private static ChestMenu menu;
    private static int age, clickAge, revision, settle;
    private static int observedRevision, stableTicks;
    private static ItemStack pending = ItemStack.EMPTY;
    private static int beforePlayer, beforeChest;
    private static boolean depositing;
    private static boolean initialReceived;
    static boolean active() { return chest != null; }
    static boolean supported(Minecraft mc, BlockPos pos) {
        if (mc.level == null || !mc.level.hasChunkAt(pos)) return false;
        var b = mc.level.getBlockState(pos).getBlock();
        return b == Blocks.BARREL || b instanceof ChestBlock && !(b instanceof TrappedChestBlock);
    }
    static boolean owned(Minecraft mc) {
        if (chest == null || mc.player == null || !supported(mc, chest)) return false;
        if (!(mc.screen instanceof AbstractContainerScreen<?> screen) ||
                !(mc.player.containerMenu instanceof ChestMenu current) || screen.getMenu() != current) return false;
        // Only adopt the menu created immediately after our checked useItemOn call.
        return menu != null ? current == menu : age <= 60;
    }
    static void begin(Minecraft mc, BlockPos pos) {
        if (!HomeSettings.storageArea(mc, pos) || !supported(mc, pos))
            throw new IllegalStateException("Chest is not supported or outside your home search area.");
        Vec3 center = Vec3.atCenterOf(pos);
        if (mc.player.getEyePosition().distanceTo(center) > Math.min(4.5, mc.gameMode.getPickRange()))
            throw new IllegalStateException("Chest is out of reach.");
        var hit = mc.level.clip(new ClipContext(mc.player.getEyePosition(), center,
            ClipContext.Block.OUTLINE, ClipContext.Fluid.NONE, mc.player));
        if (hit.getType() != HitResult.Type.BLOCK || !hit.getBlockPos().equals(pos))
            throw new IllegalStateException("Chest is hidden behind a block.");
        if (!mc.player.onGround() || mc.player.isInWater() || mc.player.isInLava())
            throw new IllegalStateException("Chest transfer requires dry supported ground.");
        chest = pos.immutable(); age = 0; menu = null; pending = ItemStack.EMPTY; settle = 0; initialReceived = false;
        mc.player.setShiftKeyDown(false);
        var result = mc.gameMode.useItemOn(mc.player, InteractionHand.MAIN_HAND, hit);
        if (!result.consumesAction()) { cancel(mc); throw new IllegalStateException("Game refused to open chest."); }
    }
    static void cancel(Minecraft mc) {
        // Closing a menu with a carried stack can drop it if inventory is full. Leave it for the user.
        boolean close = owned(mc) && mc.player.containerMenu.getCarried().isEmpty();
        chest = null; menu = null; pending = ItemStack.EMPTY;
        if (close) mc.player.closeContainer();
    }
    static boolean keep(Minecraft mc, ItemStack stack) {
        return PickaxeCraft.keepMaterial(mc,stack) || ClientBridge.matches(mc, stack, "pickaxe") || ClientBridge.matches(mc, stack, "sword") ||
            ClientBridge.matches(mc, stack, "food") || ClientBridge.matches(mc, stack, "torch");
    }
    private static boolean reservePick(Minecraft mc, ItemStack stack) {
        return ClientBridge.matches(mc, stack, "pickaxe") &&
            (!stack.isDamageableItem() || stack.getMaxDamage() - stack.getDamageValue() > 32) &&
            ClientBridge.harvestsTarget(mc, stack);
    }
    private static int count(Minecraft mc, String kind) {
        return mc.player.getInventory().items.stream().filter(s -> ClientBridge.matches(mc, s, kind)).mapToInt(ItemStack::getCount).sum();
    }
    private static boolean needed(Minecraft mc, ItemStack stack) {
        return PickaxeCraft.needsMaterial(mc,stack) || reservePick(mc, stack) && mc.player.getInventory().items.stream().noneMatch(s -> reservePick(mc, s)) ||
            ClientBridge.matches(mc, stack, "food") && count(mc, "food") < 8 ||
            ClientBridge.matches(mc, stack, "sword") && count(mc, "sword") == 0 ||
            ClientBridge.matches(mc, stack, "torch") && count(mc, "torch") < 16;
    }
    private static int total(ItemStack item, int start, int end) {
        int n = 0;
        for (int i = start; i < end; i++) {
            ItemStack s = menu.getSlot(i).getItem();
            if (ItemStack.isSameItemSameTags(item, s)) n += s.getCount();
        }
        return n;
    }
    private static boolean room(ItemStack item, int start, int end) {
        for (int i = start; i < end; i++) {
            var slot = menu.getSlot(i);
            ItemStack s = slot.getItem();
            if (slot.mayPlace(item) && (s.isEmpty() || ItemStack.isSameItemSameTags(item, s) &&
                s.getCount() < Math.min(slot.getMaxStackSize(item), item.getMaxStackSize()))) return true;
        }
        return false;
    }
    /** Returns null while busy; otherwise the verified transfer summary. */
    static String update(Minecraft mc) {
        age++;
        if (!supported(mc, chest) || !HomeSettings.storageArea(mc, chest) ||
            mc.player.position().distanceTo(Vec3.atCenterOf(chest)) > 5.)
            throw new IllegalStateException("Chest changed or player moved away.");
        if (menu == null) {
            if (owned(mc)) { menu = (ChestMenu) mc.player.containerMenu; settle = 4; }
            else if (age > 60) throw new IllegalStateException("Chest did not open (locked, blocked, or unsupported menu).");
            return null;
        }
        if (!owned(mc)) throw new IllegalStateException("Chest menu closed or changed.");
        if (!initialReceived && menu.getStateId() == 0) {
            if (age > 60) throw new IllegalStateException("Chest's initial inventory has not arrived from the server.");
            return null;
        }
        initialReceived = true;
        if (!menu.getCarried().isEmpty()) throw new IllegalStateException("Cursor holds an item; transfer stopped without dropping it.");
        int slots = menu.getRowCount() * 9;
        if (menu.slots.size() != slots + 36) throw new IllegalStateException("Unexpected chest slot layout.");
        if (!pending.isEmpty()) {
            clickAge++;
            // Wait for the server's changed slots and for all revisions in that update to settle.
            if (menu.getStateId() != observedRevision) { observedRevision = menu.getStateId(); stableTicks = 0; }
            else stableTicks++;
            if (menu.getStateId() == revision || stableTicks < 2 || clickAge < 4) {
                if (clickAge > 60) throw new IllegalStateException("No settled server acknowledgement for chest transfer.");
                return null;
            }
            int playerDelta = total(pending, slots, slots + 36) - beforePlayer;
            int chestDelta = total(pending, 0, slots) - beforeChest;
            if (playerDelta + chestDelta != 0 || (depositing ? playerDelta >= 0 : playerDelta <= 0))
                throw new IllegalStateException("Chest transfer was not confirmed; stopped to avoid repeating an uncertain click.");
            pending = ItemStack.EMPTY;
            settle = 2;
        }
        if (settle-- > 0) return null;
        for (int i = slots; i < slots + 36; i++) {
            ItemStack s = menu.getSlot(i).getItem();
            if (!s.isEmpty() && !keep(mc, s) && room(s, 0, slots)) { click(mc, i, slots, true); return null; }
        }
        for (int i = 0; i < slots; i++) {
            ItemStack s = menu.getSlot(i).getItem();
            if (!s.isEmpty() && needed(mc, s) && room(s, slots, slots + 36)) { click(mc, i, slots, false); return null; }
        }
        long left = mc.player.getInventory().items.stream().filter(s -> !s.isEmpty() && !keep(mc, s)).count();
        return "Chest transfer confirmed; remaining cargo stacks=" + left + ".";
    }
    private static void click(Minecraft mc, int slot, int chestSlots, boolean deposit) {
        pending = menu.getSlot(slot).getItem().copy();
        beforePlayer = total(pending, chestSlots, chestSlots + 36);
        beforeChest = total(pending, 0, chestSlots);
        revision = menu.getStateId(); observedRevision = revision; stableTicks = 0; clickAge = 0; depositing = deposit;
        // handleInventoryMouseClick predicts the move and tells the server those predicted slots;
        // a matching prediction may receive no reply. Omit prediction so broadcastChanges sends
        // the actual changed slots. The server still performs its normal QUICK_MOVE validation.
        if (mc.getConnection() == null) throw new IllegalStateException("Connection ended before transfer.");
        mc.getConnection().send(new net.minecraft.network.protocol.game.ServerboundContainerClickPacket(
            menu.containerId, revision, slot, 0, ClickType.QUICK_MOVE, ItemStack.EMPTY,
            new it.unimi.dsi.fastutil.ints.Int2ObjectOpenHashMap<>()));
    }
}
