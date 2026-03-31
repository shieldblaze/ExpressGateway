/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2022 ShieldBlaze
 *
 * ShieldBlaze ExpressGateway is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version.
 *
 * ShieldBlaze ExpressGateway is distributed in the hope that it will be useful,
 * but WITHOUT ANY WARRANTY; without even the implied warranty of
 * MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
 * GNU General Public License for more details.
 *
 * You should have received a copy of the GNU General Public License
 * along with ShieldBlaze ExpressGateway.  If not, see <https://www.gnu.org/licenses/>.
 */
package com.shieldblaze.expressgateway.controlplane.kvstore;

import com.shieldblaze.expressgateway.controlplane.testutil.InMemoryKVStore;
import org.junit.jupiter.api.BeforeEach;
import org.junit.jupiter.api.Test;

import java.nio.charset.StandardCharsets;
import java.util.List;
import java.util.Map;
import java.util.Optional;

import static org.junit.jupiter.api.Assertions.assertArrayEquals;
import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

class KVStoreTransactionTest {

    private InMemoryKVStore store;

    @BeforeEach
    void setUp() {
        store = new InMemoryKVStore();
    }

    @Test
    void batchPutWritesAllKeys() throws KVStoreException {
        store.batchPut(Map.of(
                "/config/a", "value-a".getBytes(),
                "/config/b", "value-b".getBytes(),
                "/config/c", "value-c".getBytes()
        ));

        assertTrue(store.get("/config/a").isPresent());
        assertTrue(store.get("/config/b").isPresent());
        assertTrue(store.get("/config/c").isPresent());
    }

    @Test
    void batchGetReturnsFoundKeys() throws KVStoreException {
        store.put("/config/a", "a".getBytes());
        store.put("/config/b", "b".getBytes());

        List<KVEntry> results = store.batchGet(List.of("/config/a", "/config/missing", "/config/b"));
        assertEquals(2, results.size());
    }

    @Test
    void casTransactionSucceedsWithCorrectVersions() throws KVStoreException {
        long verA = store.put("/config/a", "old-a".getBytes());
        long verB = store.put("/config/b", "old-b".getBytes());

        store.casTransaction(
                Map.of("/config/a", verA, "/config/b", verB),
                Map.of("/config/a", "new-a".getBytes(), "/config/b", "new-b".getBytes())
        );

        Optional<KVEntry> a = store.get("/config/a");
        assertTrue(a.isPresent());
        assertArrayEquals("new-a".getBytes(), a.get().value());
    }

    @Test
    void casTransactionFailsOnVersionMismatch() throws KVStoreException {
        long verA = store.put("/config/a", "old-a".getBytes());
        store.put("/config/b", "old-b".getBytes());

        KVStoreException ex = assertThrows(KVStoreException.class, () ->
                store.casTransaction(
                        Map.of("/config/a", verA, "/config/b", 999L), // wrong version for b
                        Map.of("/config/a", "new-a".getBytes())
                )
        );
        assertEquals(KVStoreException.Code.VERSION_CONFLICT, ex.code());
    }

    @Test
    void casTransactionWithExpectedAbsentKeyFails() throws KVStoreException {
        store.put("/config/a", "exists".getBytes());

        KVStoreException ex = assertThrows(KVStoreException.class, () ->
                store.casTransaction(
                        Map.of("/config/a", 0L), // 0 = must not exist
                        Map.of("/config/a", "new".getBytes())
                )
        );
        assertEquals(KVStoreException.Code.VERSION_CONFLICT, ex.code());
    }

    @Test
    void casTransactionCreateIfAbsentSucceeds() throws KVStoreException {
        store.casTransaction(
                Map.of("/config/new-key", 0L),
                Map.of("/config/new-key", "created".getBytes())
        );

        assertTrue(store.get("/config/new-key").isPresent());
    }

    @Test
    void batchPutRespectsFailOnWrite() {
        store.setFailOnWrite(true);
        assertThrows(KVStoreException.class, () ->
                store.batchPut(Map.of("/config/a", "a".getBytes()))
        );
    }
}
