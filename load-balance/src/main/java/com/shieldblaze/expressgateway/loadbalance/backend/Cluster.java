package com.shieldblaze.expressgateway.backend;

import java.util.ArrayList;
import java.util.List;

public final class Cluster {
    private final List<Backend> backends = new ArrayList<>();

    private String clusterName;
}
