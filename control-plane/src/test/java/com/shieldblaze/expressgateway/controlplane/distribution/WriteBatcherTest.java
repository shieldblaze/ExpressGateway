package com.shieldblaze.expressgateway.controlplane.distribution;

import com.shieldblaze.expressgateway.controlplane.config.ConfigKind;
import com.shieldblaze.expressgateway.controlplane.config.ConfigMutation;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResource;
import com.shieldblaze.expressgateway.controlplane.config.ConfigResourceId;
import com.shieldblaze.expressgateway.controlplane.config.ConfigScope;
import com.shieldblaze.expressgateway.controlplane.config.ConfigSpec;
import org.junit.jupiter.api.AfterEach;
import org.junit.jupiter.api.Test;

import java.time.Instant;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Map;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.atomic.AtomicInteger;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class WriteBatcherTest {

    record TestSpec(String value) implements ConfigSpec {
        @Override
        public void validate() { }
    }

    private WriteBatcher batcher;

    @AfterEach
    void tearDown() {
        if (batcher != null) {
            batcher.close();
        }
    }

    private ConfigMutation.Upsert upsert(String name) {
        ConfigResourceId id = new ConfigResourceId("cluster", "global", name);
        ConfigResource resource = new ConfigResource(
                id,
                ConfigKind.CLUSTER,
                new ConfigScope.Global(),
                1,
                Instant.now(),
                Instant.now(),
                "admin",
                Map.of(),
                new TestSpec(name)
        );
        return new ConfigMutation.Upsert(resource);
    }

    @Test
    void flushNowDrainsAllSubmittedMutationsIntoSingleBatch() {
        List<List<ConfigMutation>> flushedBatches = new CopyOnWriteArrayList<>();
        batcher = new WriteBatcher(1000, flushedBatches::add);
        batcher.start();

        batcher.submit(upsert("a"));
        batcher.submit(upsert("b"));
        batcher.submit(upsert("c"));

        batcher.flushNow();

        assertEquals(1, flushedBatches.size(), "All mutations should be in a single batch");
        assertEquals(3, flushedBatches.get(0).size());
    }

    @Test
    void multipleMutationsCoalescedIntoSingleBatch() {
        List<List<ConfigMutation>> flushedBatches = new CopyOnWriteArrayList<>();
        batcher = new WriteBatcher(1000, flushedBatches::add);
        batcher.start();

        // Submit 5 mutations individually
        for (int i = 0; i < 5; i++) {
            batcher.submit(upsert("resource-" + i));
        }

        batcher.flushNow();

        assertEquals(1, flushedBatches.size(), "All mutations should be coalesced");
        assertEquals(5, flushedBatches.get(0).size());
    }

    @Test
    void emptyQueueProducesNoCallbackInvocation() {
        AtomicInteger callbackCount = new AtomicInteger(0);
        batcher = new WriteBatcher(1000, batch -> callbackCount.incrementAndGet());
        batcher.start();

        batcher.flushNow();

        assertEquals(0, callbackCount.get(), "Callback should not be invoked for empty queue");
    }

    @Test
    void flushCallbackFailureRequeuesMutationsAtHeadInOriginalOrder() {
        AtomicInteger callCount = new AtomicInteger(0);
        List<List<ConfigMutation>> successfulBatches = new CopyOnWriteArrayList<>();

        batcher = new WriteBatcher(1000, batch -> {
            if (callCount.getAndIncrement() == 0) {
                throw new RuntimeException("Simulated flush failure");
            }
            successfulBatches.add(new ArrayList<>(batch));
        });
        batcher.start();

        ConfigMutation m1 = upsert("first");
        ConfigMutation m2 = upsert("second");
        ConfigMutation m3 = upsert("third");

        batcher.submit(m1);
        batcher.submit(m2);
        batcher.submit(m3);

        // First flush fails -- mutations should be re-queued
        batcher.flushNow();
        assertEquals(0, successfulBatches.size(), "First flush should fail");

        // Second flush succeeds -- re-queued mutations should arrive in original order
        batcher.flushNow();
        assertEquals(1, successfulBatches.size(), "Second flush should succeed");
        List<ConfigMutation> retried = successfulBatches.get(0);
        assertEquals(3, retried.size());
        // Verify original ordering is preserved after re-queue
        assertEquals(m1, retried.get(0));
        assertEquals(m2, retried.get(1));
        assertEquals(m3, retried.get(2));
    }

    @Test
    void flushCallbackFailurePreservesOrderWithNewSubmissions() {
        AtomicInteger callCount = new AtomicInteger(0);
        List<List<ConfigMutation>> successfulBatches = new CopyOnWriteArrayList<>();

        batcher = new WriteBatcher(1000, batch -> {
            if (callCount.getAndIncrement() == 0) {
                throw new RuntimeException("Simulated flush failure");
            }
            successfulBatches.add(new ArrayList<>(batch));
        });
        batcher.start();

        ConfigMutation m1 = upsert("first");
        ConfigMutation m2 = upsert("second");

        batcher.submit(m1);
        batcher.submit(m2);

        // First flush fails
        batcher.flushNow();

        // Submit a new mutation after the failure
        ConfigMutation m3 = upsert("third");
        batcher.submit(m3);

        // Second flush succeeds -- re-queued mutations should be at the head, new one at the tail
        batcher.flushNow();
        assertEquals(1, successfulBatches.size());
        List<ConfigMutation> retried = successfulBatches.get(0);
        assertEquals(3, retried.size());
        assertEquals(m1, retried.get(0), "Re-queued mutations should appear first");
        assertEquals(m2, retried.get(1));
        assertEquals(m3, retried.get(2), "Newly submitted mutation should appear last");
    }

    @Test
    void closeDrainsRemainingMutations() {
        List<List<ConfigMutation>> flushedBatches = new CopyOnWriteArrayList<>();
        batcher = new WriteBatcher(60_000, flushedBatches::add); // Very long window so scheduler won't fire
        batcher.start();

        batcher.submit(upsert("a"));
        batcher.submit(upsert("b"));

        // close() should drain
        batcher.close();
        batcher = null; // Prevent double-close in tearDown

        assertTrue(flushedBatches.size() >= 1, "close() should have drained remaining mutations");
        int totalFlushed = flushedBatches.stream().mapToInt(List::size).sum();
        assertEquals(2, totalFlushed);
    }

    @Test
    void invalidBatchWindowZeroThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class,
                () -> new WriteBatcher(0, batch -> { }));
    }

    @Test
    void invalidBatchWindowNegativeThrowsIllegalArgumentException() {
        assertThrows(IllegalArgumentException.class,
                () -> new WriteBatcher(-100, batch -> { }));
    }

    @Test
    void nullCallbackThrowsNullPointerException() {
        assertThrows(NullPointerException.class,
                () -> new WriteBatcher(500, null));
    }

    @Test
    void submitNullMutationThrowsNullPointerException() {
        batcher = new WriteBatcher(1000, batch -> { });
        batcher.start();
        assertThrows(NullPointerException.class, () -> batcher.submit(null));
    }

    @Test
    void submitAllNullListThrowsNullPointerException() {
        batcher = new WriteBatcher(1000, batch -> { });
        batcher.start();
        assertThrows(NullPointerException.class, () -> batcher.submitAll(null));
    }

    @Test
    void submitAllAddsMutationsInOrder() {
        List<List<ConfigMutation>> flushedBatches = new CopyOnWriteArrayList<>();
        batcher = new WriteBatcher(1000, flushedBatches::add);
        batcher.start();

        ConfigMutation m1 = upsert("x");
        ConfigMutation m2 = upsert("y");
        ConfigMutation m3 = upsert("z");

        batcher.submitAll(List.of(m1, m2, m3));

        batcher.flushNow();

        assertEquals(1, flushedBatches.size());
        List<ConfigMutation> batch = flushedBatches.get(0);
        assertEquals(3, batch.size());
        assertEquals(m1, batch.get(0));
        assertEquals(m2, batch.get(1));
        assertEquals(m3, batch.get(2));
    }

    @Test
    void submitAllWithEmptyListProducesNoCallbackOnFlush() {
        AtomicInteger callbackCount = new AtomicInteger(0);
        batcher = new WriteBatcher(1000, batch -> callbackCount.incrementAndGet());
        batcher.start();

        batcher.submitAll(Collections.emptyList());
        batcher.flushNow();

        assertEquals(0, callbackCount.get());
    }

    @Test
    void multipleFlushNowCallsWithInterleavedSubmissions() {
        List<List<ConfigMutation>> flushedBatches = new CopyOnWriteArrayList<>();
        batcher = new WriteBatcher(1000, flushedBatches::add);
        batcher.start();

        batcher.submit(upsert("a"));
        batcher.flushNow();

        batcher.submit(upsert("b"));
        batcher.submit(upsert("c"));
        batcher.flushNow();

        batcher.flushNow(); // Empty -- should not produce a callback

        assertEquals(2, flushedBatches.size());
        assertEquals(1, flushedBatches.get(0).size());
        assertEquals(2, flushedBatches.get(1).size());
    }
}
