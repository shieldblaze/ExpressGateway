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
import com.shieldblaze.expressgateway.backend.events.BackendEvent;
import com.shieldblaze.expressgateway.backend.exceptions.BackendNotOnlineException;
import com.shieldblaze.expressgateway.common.concurrent.GlobalExecutors;
import com.shieldblaze.expressgateway.common.eventstream.AsyncEventStream;
import com.shieldblaze.expressgateway.common.eventstream.EventListener;
import com.shieldblaze.expressgateway.common.eventstream.EventStream;

import java.util.List;
import java.util.Objects;
import java.util.concurrent.CopyOnWriteArrayList;
import java.util.concurrent.Executors;
import java.util.stream.Collectors;
import java.util.stream.Stream;

/**
 * Base class for Cluster
 */
public abstract class Cluster {

    /**
     * Stream of {@linkplain BackendEvent}
     */
    private final AsyncEventStream eventStream = new AsyncEventStream(GlobalExecutors.INSTANCE.getExecutorService());

    /**
     * List of all {@linkplain Backend} associated with this {@linkplain Cluster}
     */
    protected final List<Backend> allBackends = new CopyOnWriteArrayList<>();

    /**
     * Name of this {@linkplain Cluster}
     */
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
    }

    /**
     * Get {@linkplain List} of online {@linkplain Backend} in this {@linkplain Cluster}
     */
    public List<Backend> getOnlineBackends() {
        return allBackends.stream()
                .filter(backend -> backend.getState() == State.ONLINE)
                .collect(Collectors.toList());
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
    public Backend getOnline(int index) throws BackendNotOnlineException {
        Backend backend = allBackends.get(index);
        if (backend.getState() != State.ONLINE) {
            throw new BackendNotOnlineException(backend);
        }
        return backend;
    }

    /**
     * Get size of this {@linkplain Cluster}
     */
    public int size() {
        return allBackends.size();
    }

    /**
     * Get number of Online {@linkplain Backend} in this {@linkplain Cluster}
     */
    public int online() {
        return (int) allBackends.stream().filter(backend -> backend.getState() == State.ONLINE).count();
    }

    /**
     * Subscribe to stream of {@link BackendEvent}
     *
     * @see EventStream#subscribe(EventListener)
     */
    public void subscribeStream(EventListener eventListener) {
        eventStream.subscribe(eventListener);
    }

    /**
     * Unsubscribe from stream of {@link BackendEvent}
     *
     * @see EventStream#subscribe(EventListener)
     */
    public void unsubscribeStream(EventListener eventListener) {
        eventStream.unsubscribe(eventListener);
    }
}
