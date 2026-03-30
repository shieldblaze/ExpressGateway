package com.shieldblaze.expressgateway.controlplane.agent;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertDoesNotThrow;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class ReconnectStrategyTest {

    @Test
    void defaultConstructorCreatesValidInstance() {
        assertDoesNotThrow(() -> new ReconnectStrategy());
    }

    @Test
    void firstDelayIsInBaseRange() {
        // With base=1000, first attempt upper bound = min(max, 1000 * 2^0) = 1000
        // Full jitter: [0, 1000)
        ReconnectStrategy strategy = new ReconnectStrategy(1000, 30_000);
        for (int i = 0; i < 100; i++) {
            strategy.reset();
            long delay = strategy.nextDelay();
            assertTrue(delay >= 0, "delay must be >= 0, got: " + delay);
            assertTrue(delay < 1000, "first delay must be < baseDelayMs (1000), got: " + delay);
        }
    }

    @Test
    void delaysGrowExponentially() {
        // Use a large maxDelayMs so the cap doesn't interfere.
        // Exponential upper bounds: base*2^0, base*2^1, base*2^2, ...
        // i.e., 100, 200, 400, 800, ...
        // After many samples the max observed delay at attempt N should be > max at attempt N-1
        long base = 100;
        long max = 1_000_000;
        int samples = 500;

        // Collect the maximum observed delay at each attempt level
        long[] maxObserved = new long[6];
        for (int s = 0; s < samples; s++) {
            ReconnectStrategy strategy = new ReconnectStrategy(base, max);
            for (int attempt = 0; attempt < maxObserved.length; attempt++) {
                long delay = strategy.nextDelay();
                if (delay > maxObserved[attempt]) {
                    maxObserved[attempt] = delay;
                }
            }
        }

        // The upper bound doubles each attempt, so the max observed should generally increase.
        // At attempt 0: upper bound = 100, at attempt 1: 200, at attempt 2: 400, etc.
        // With 500 samples, the max observed at higher attempts should exceed earlier ones.
        for (int i = 1; i < maxObserved.length; i++) {
            assertTrue(maxObserved[i] >= maxObserved[i - 1],
                    "Max observed delay at attempt " + i + " (" + maxObserved[i] +
                            ") should be >= attempt " + (i - 1) + " (" + maxObserved[i - 1] + ")");
        }

        // Additionally verify that later attempts produce strictly larger upper bounds in theory.
        // The max observed at attempt 5 (upper bound 3200) should exceed attempt 0 (upper bound 100).
        assertTrue(maxObserved[5] > maxObserved[0],
                "Max at attempt 5 (" + maxObserved[5] + ") should exceed max at attempt 0 (" + maxObserved[0] + ")");
    }

    @Test
    void delayNeverExceedsMaxDelayMs() {
        long base = 1000;
        long max = 5000;
        ReconnectStrategy strategy = new ReconnectStrategy(base, max);

        for (int i = 0; i < 100; i++) {
            long delay = strategy.nextDelay();
            assertTrue(delay >= 0, "delay must be >= 0, got: " + delay);
            assertTrue(delay < max, "delay must be < maxDelayMs (" + max + "), got: " + delay);
        }
    }

    @Test
    void resetResetsToInitialBehavior() {
        ReconnectStrategy strategy = new ReconnectStrategy(1000, 30_000);

        // Advance several attempts
        for (int i = 0; i < 10; i++) {
            strategy.nextDelay();
        }

        // Reset and verify first delay is back in initial range
        strategy.reset();
        for (int i = 0; i < 100; i++) {
            strategy.reset();
            long delay = strategy.nextDelay();
            assertTrue(delay >= 0, "delay must be >= 0 after reset, got: " + delay);
            assertTrue(delay < 1000, "first delay after reset must be < baseDelayMs (1000), got: " + delay);
        }
    }

    @Test
    void delayIsAlwaysNonNegative() {
        ReconnectStrategy strategy = new ReconnectStrategy(1, 1);
        for (int i = 0; i < 1000; i++) {
            long delay = strategy.nextDelay();
            assertTrue(delay >= 0, "delay must always be >= 0, got: " + delay);
        }
    }

    @Test
    void afterManyAttemptsDelayStaysBounded() {
        long max = 30_000;
        ReconnectStrategy strategy = new ReconnectStrategy(1000, max);

        // Advance well past the shift cap of 20
        for (int i = 0; i < 50; i++) {
            long delay = strategy.nextDelay();
            assertTrue(delay >= 0, "delay must be >= 0 at attempt " + i + ", got: " + delay);
            assertTrue(delay < max, "delay must be < maxDelayMs (" + max + ") at attempt " + i + ", got: " + delay);
        }
    }

    @Test
    void baseDelayMsZeroThrows() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStrategy(0, 30_000));
        assertTrue(ex.getMessage().contains("baseDelayMs"), "message should mention baseDelayMs");
    }

    @Test
    void baseDelayMsNegativeThrows() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStrategy(-1, 30_000));
        assertTrue(ex.getMessage().contains("baseDelayMs"), "message should mention baseDelayMs");
    }

    @Test
    void maxDelayMsZeroThrows() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStrategy(1000, 0));
        assertTrue(ex.getMessage().contains("maxDelayMs"), "message should mention maxDelayMs");
    }

    @Test
    void maxDelayMsNegativeThrows() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStrategy(1000, -5));
        assertTrue(ex.getMessage().contains("maxDelayMs"), "message should mention maxDelayMs");
    }

    @Test
    void maxDelayMsLessThanBaseDelayMsThrows() {
        IllegalArgumentException ex = assertThrows(IllegalArgumentException.class,
                () -> new ReconnectStrategy(5000, 1000));
        assertTrue(ex.getMessage().contains("maxDelayMs"), "message should mention maxDelayMs");
    }

    @Test
    void equalBaseAndMaxDelayIsValid() {
        // When base == max, the exponential delay is always capped at max.
        // Full jitter: [0, max). All delays should be in [0, max).
        long value = 500;
        ReconnectStrategy strategy = new ReconnectStrategy(value, value);
        for (int i = 0; i < 100; i++) {
            long delay = strategy.nextDelay();
            assertTrue(delay >= 0, "delay must be >= 0, got: " + delay);
            assertTrue(delay < value, "delay must be < " + value + ", got: " + delay);
        }
    }
}
