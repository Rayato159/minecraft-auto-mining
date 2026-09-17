package dev.lookhin.flyminer;

import net.minecraft.core.BlockPos;
import java.util.Comparator;
import java.util.function.Predicate;
import java.util.function.ToDoubleFunction;
import java.util.stream.Stream;

final class StationSearch {
    private StationSearch() {}

    static BlockPos nearest(Stream<BlockPos> positions, Predicate<BlockPos> eligible,
                            ToDoubleFunction<BlockPos> distance) {
        // Minecraft's betweenClosedStream reuses one MutableBlockPos. A reduction
        // must own immutable values before retaining any candidate across steps.
        return positions.map(BlockPos::immutable).filter(eligible)
            .min(Comparator.comparingDouble(distance)).orElse(null);
    }
}
