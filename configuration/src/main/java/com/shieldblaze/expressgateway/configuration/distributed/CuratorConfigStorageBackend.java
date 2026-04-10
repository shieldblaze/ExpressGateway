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
package com.shieldblaze.expressgateway.configuration.distributed;

import com.shieldblaze.expressgateway.common.zookeeper.Curator;
import org.apache.curator.framework.CuratorFramework;
import org.apache.curator.framework.recipes.cache.CuratorCache;
import org.apache.curator.framework.recipes.cache.CuratorCacheListener;
import org.apache.curator.framework.recipes.leader.LeaderLatch;
import org.apache.curator.framework.recipes.leader.LeaderLatchListener;
import org.apache.curator.framework.state.ConnectionState;
import org.apache.curator.framework.state.ConnectionStateListener;
import org.apache.logging.log4j.LogManager;
import org.apache.logging.log4j.Logger;
import org.apache.zookeeper.CreateMode;
import org.apache.zookeeper.KeeperException;

import java.io.Closeable;
import java.io.IOException;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;
import java.util.Optional;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.function.Consumer;

/**
 * Curator/ZooKeeper-backed implementation of {@link ConfigStorageBackend}.
 *
 * <p>This is the default backend used when no external {@link ConfigStorageBackend} is
 * provided to the distributed configuration subsystem. It delegates directly to
 * {@link Curator#getInstance()} for backward compatibility.</p>
 */
final class CuratorConfigStorageBackend implements ConfigStorageBackend {

    private static final Logger logger = LogManager.getLogger(CuratorConfigStorageBackend.class);

    private final List<Runnable> connectionLossListeners = new CopyOnWriteArrayList<>();
    private final ConnectionStateListener curatorConnectionListener;

    CuratorConfigStorageBackend() {
        // Register a Curator ConnectionStateListener that delegates to our connection loss listeners
        this.curatorConnectionListener = (_, newState) -> {
            if (newState == ConnectionState.LOST || newState == ConnectionState.SUSPENDED) {
                for (Runnable listener : connectionLossListeners) {
                    try {
                        listener.run();
                    } catch (Exception e) {
                        logger.warn("Connection loss listener threw exception", e);
                    }
                }
            }
        };
        try {
            Curator.getInstance().getConnectionStateListenable().addListener(curatorConnectionListener);
        } catch (Exception e) {
            logger.warn("Failed to register ConnectionStateListener on Curator", e);
        }
    }

    @Override
    public Optional<byte[]> get(String key) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        if (curator.checkExists().forPath(key) == null) {
            return Optional.empty();
        }
        return Optional.of(curator.getData().forPath(key));
    }

    @Override
    public void put(String key, byte[] value) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        curator.create()
                .orSetData()
                .creatingParentsIfNeeded()
                .withMode(CreateMode.PERSISTENT)
                .forPath(key, value);
    }

    @Override
    public void putIfAbsent(String key, byte[] value) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        try {
            curator.create()
                    .creatingParentsIfNeeded()
                    .withMode(CreateMode.PERSISTENT)
                    .forPath(key, value);
        } catch (KeeperException.NodeExistsException e) {
            throw new KeyExistsException(key, e);
        }
    }

    @Override
    public String putSequential(String keyPrefix, byte[] value) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        // Ensure parent exists
        String parentPath = keyPrefix.substring(0, keyPrefix.lastIndexOf('/'));
        curator.create()
                .orSetData()
                .creatingParentsIfNeeded()
                .withMode(CreateMode.PERSISTENT)
                .forPath(parentPath, new byte[0]);

        return curator.create()
                .withMode(CreateMode.PERSISTENT_SEQUENTIAL)
                .forPath(keyPrefix, value);
    }

    @Override
    public void putEphemeral(String key, byte[] value) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        // Ensure parent path exists
        String parentPath = key.substring(0, key.lastIndexOf('/'));
        curator.create()
                .orSetData()
                .creatingParentsIfNeeded()
                .withMode(CreateMode.PERSISTENT)
                .forPath(parentPath, new byte[0]);

        try {
            curator.create()
                    .withMode(CreateMode.EPHEMERAL)
                    .forPath(key, value);
        } catch (KeeperException.NodeExistsException e) {
            throw new KeyExistsException(key, e);
        }
    }

    @Override
    public boolean exists(String key) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        return curator.checkExists().forPath(key) != null;
    }

    @Override
    public List<String> listChildren(String key) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        if (curator.checkExists().forPath(key) == null) {
            return Collections.emptyList();
        }
        return curator.getChildren().forPath(key);
    }

    @Override
    public void delete(String key) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        curator.delete().forPath(key);
    }

    @Override
    public void deleteTree(String key) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        if (curator.checkExists().forPath(key) != null) {
            curator.delete().deletingChildrenIfNeeded().forPath(key);
        }
    }

    @Override
    public Closeable watch(String path, WatchListener listener) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        CuratorCache cache = CuratorCache.build(curator, path);

        CuratorCacheListener cacheListener = CuratorCacheListener.builder()
                .forChanges((_, newNode) -> listener.onDataChanged(newNode.getData()))
                .forCreates(childData -> listener.onDataChanged(childData.getData()))
                .build();

        cache.listenable().addListener(cacheListener);
        cache.start();

        return cache::close;
    }

    @Override
    public LeaderElection leaderElection(String electionPath, String participantId) throws Exception {
        CuratorFramework curator = Curator.getInstance();
        LeaderLatch latch = new LeaderLatch(curator, electionPath, participantId);

        return new LeaderElection() {
            @Override
            public void start() throws Exception {
                latch.start();
            }

            @Override
            public boolean isLeader() {
                return latch.hasLeadership();
            }

            @Override
            public void addListener(Consumer<Boolean> listener) {
                latch.addListener(new LeaderLatchListener() {
                    @Override
                    public void isLeader() {
                        listener.accept(true);
                    }

                    @Override
                    public void notLeader() {
                        listener.accept(false);
                    }
                });
            }

            @Override
            public void close() throws IOException {
                latch.close();
            }
        };
    }

    @Override
    public void addConnectionLossListener(Runnable listener) {
        connectionLossListeners.add(listener);
    }

    @Override
    public void removeConnectionLossListener(Runnable listener) {
        connectionLossListeners.remove(listener);
    }
}
