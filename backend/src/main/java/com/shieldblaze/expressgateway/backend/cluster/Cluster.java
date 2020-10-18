/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020 ShieldBlaze
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
package com.shieldblaze.expressgateway.backend.cluster;

import com.shieldblaze.expressgateway.backend.Backend;
import com.shieldblaze.expressgateway.backend.State;

import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.stream.Collectors;
import java.util.stream.Stream;

/**
 * Base class for Cluster
 */
public abstract class Cluster {
    protected final List<Backend> allBackends = new CopyOnWriteArrayList<>();
    private final List<Backend> onlineBackends = new CopyOnWriteArrayList<>();

    private int roundRobinIndex = 0;

    private String name;

    /**
     * Get name of this {@linkplain Cluster}
     *
     * @return Name as {@link String}
     */
    public String getName() {
        return name;
    }

    /**
     * Set name of this {@linkplain Cluster}
     *
     * @param name Name as {@link String}
     */
    public void setName(String name) {
        this.name = Objects.requireNonNull(name, "name");
    }

    /**
     * Add {@link Backend} into this {@linkplain Cluster}
     */
    protected void addBackend(Backend backend) {
        allBackends.add(Objects.requireNonNull(backend, "backend"));
        onlineBackends.add(backend);
        roundRobinIndex = 0;
    }

    /**
     * Get {@linkplain List} of available (Online) {@linkplain Backend} in this {@linkplain Cluster}
     */
    public List<Backend> getAvailableBackends() {
        return allBackends.stream()
                .filter(backend -> backend.getState() == State.ONLINE)
                .collect(Collectors.toList());
    }

    /**
     * Get {@link Stream} of available (Online) {@linkplain Backend} in this {@linkplain Cluster}
     */
    public Stream<Backend> stream() {
        return allBackends.stream().filter(backend -> backend.getState() == State.ONLINE);
    }

    public Backend get(int index) {
        return allBackends.get(index);
    }

    /**
     * Get {@linkplain Backend} from online pool using Index
     *
     * @param index Index
     * @return {@linkplain Backend} Instance if found else {@code null}
     */
    public Backend getOnline(int index) {
        return allBackends.get(index);
    }

    /**
     * [ROUND ROBIN] Get the next {@linkplain Backend}
     */
    public Backend next() {
        try {
            if (roundRobinIndex >= onlineBackends.size()) {
                roundRobinIndex = 0;
            }
            return onlineBackends.get(roundRobinIndex);
        } catch (Exception ex) {
            return null;
        } finally {
            roundRobinIndex++;
        }
    }

    /**
     * Get size of this {@linkplain Cluster}
     */
    public int size() {
        return allBackends.size();
    }

    /**
     * Get number of available (Online) {@linkplain Backend} in this {@linkplain Cluster}
     */
    public int available() {
        return (int) stream()
                .filter(backend -> backend.getState() == State.ONLINE)
                .count();
    }
}
