package com.shieldblaze.expressgateway.core.loadbalance.backend;

import java.net.InetSocketAddress;
import java.util.ArrayList;
import java.util.Collections;
import java.util.List;

/**
 * {@link Cluster} is just pool of Backends.
 */
public final class Cluster {
    private final List<Backend> backends = Collections.synchronizedList(new ArrayList<>());

    /**
     * Name of Cluster
     */
    private String clusterName;

    public List<Backend> getBackends() {
        return backends;
    }

    /**
     * Get Name of Cluster
     */
    public String getClusterName() {
        return clusterName;
    }

    /**
     * Set Name of Cluster
     */
    public void setClusterName(String clusterName) {
        this.clusterName = clusterName;
    }

    /**
     * Add {@link Backend} into this {@link Cluster}
     */
    public void addBackend(Backend backend) {
        backends.add(backend);
    }

    /**
     * Remote {@link Backend} from this {@link Cluster}
     *
     * @param socketAddress {@link InetSocketAddress} of {@link Backend}
     * @return {@code true} if {@link Backend} is successfully removed else {@code false}
     */
    public boolean removeBackend(InetSocketAddress socketAddress) {
        return backends.removeIf(backend -> backend.getSocketAddress().equals(socketAddress));
    }

    /**
     * Remote {@link Backend} from this {@link Cluster}
     *
     * @return {@code true} if {@link Backend} is successfully removed else {@code false}
     */
    public boolean removeBackend(Backend backend) {
        return backends.remove(backend);
    }

    @Override
    public String toString() {
        return "Cluster{" +
                "backends=" + backends +
                ", clusterName='" + clusterName + '\'' +
                '}';
    }
}
