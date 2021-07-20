/*
 * This file is part of ShieldBlaze ExpressGateway. [www.shieldblaze.com]
 * Copyright (c) 2020-2021 ShieldBlaze
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

package com.shieldblaze.expressgateway.integration.aws.ec2.instance;

import software.amazon.awssdk.services.ec2.model.InstanceType;

import java.util.List;
import java.util.Objects;

/**
 * Builder for {@link ScaleOutRequest}
 */
public final class ScaleOutRequestBuilder {
    private String imageId;
    private String subnetId;
    private InstanceType instanceType;
    private List<String> securityGroups;
    private boolean autoscaled;

    private ScaleOutRequestBuilder() {
        // Prevent outside initialization
    }

    public static ScaleOutRequestBuilder newBuilder() {
        return new ScaleOutRequestBuilder();
    }

    public ScaleOutRequestBuilder withImageId(String imageId) {
        this.imageId = imageId;
        return this;
    }

    public ScaleOutRequestBuilder withSubnetId(String subnetId) {
        this.subnetId = subnetId;
        return this;
    }

    public ScaleOutRequestBuilder withInstanceType(InstanceType instanceType) {
        this.instanceType = instanceType;
        return this;
    }

    public ScaleOutRequestBuilder withSecurityGroups(List<String> securityGroups) {
        this.securityGroups = securityGroups;
        return this;
    }

    public ScaleOutRequestBuilder withAutoscaled(boolean autoscaled) {
        this.autoscaled = autoscaled;
        return this;
    }

    public ScaleOutRequest build() {
        Objects.requireNonNull(imageId, "ImageID");
        Objects.requireNonNull(subnetId, "SubnetID");
        Objects.requireNonNull(instanceType, "InstanceType");
        Objects.requireNonNull(securityGroups, "SecurityGroups");

        return new ScaleOutRequest(imageId, subnetId, instanceType, securityGroups, autoscaled);
    }
}
