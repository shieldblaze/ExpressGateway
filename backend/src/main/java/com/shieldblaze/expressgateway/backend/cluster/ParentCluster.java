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

import com.shieldblaze.expressgateway.backend.loadbalance.LoadBalance;

import java.util.List;
import java.util.concurrent.CopyOnWriteArrayList;

public class ParentCluster {
    private final List<Cluster> childClusters = new CopyOnWriteArrayList<>();
    private LoadBalance<?, ?, ?, ?> loadBalance;

    public ParentCluster addChild(Cluster cluster) {
        childClusters.add(cluster);
        return this;
    }

    public boolean removeChild(Cluster cluster) {
        return childClusters.remove(cluster);
    }

    public ParentCluster loadBalance(LoadBalance<?, ?, ?, ?> loadBalance) {
        this.loadBalance = loadBalance;
        return this;
    }

    public LoadBalance<?, ?, ?, ?> loadBalance() {
        return loadBalance;
    }
}
