package com.shieldblaze.expressgateway.core.loadbalance.backend;

import java.util.ArrayList;
import java.util.List;

/**
 * Cluster is just pool of Backends.
 */
public final class Cluster {
    private final List<Backend> backends = new ArrayList<>();

    private String clusterName;

    public List<Backend> getBackends() {
        return backends;
    }

    public String getClusterName() {
        return clusterName;
    }

    public void setClusterName(String clusterName) {
        this.clusterName = clusterName;
    }

    public void addBackend(Backend backend) {
        backends.add(backend);
    }

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
