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

package com.shieldblaze.expressgateway.integration.aws.lightsail;

import java.util.Objects;

/**
 * Builder for {@link ScaleOutRequest}
 */
public final class ScaleOutRequestBuilder {

    private String instanceName;
    private String availabilityZone;
    private Bundle bundle;
    private boolean autoscaled;

    private ScaleOutRequestBuilder() {
        // Prevent outside initialization
    }

    public static ScaleOutRequestBuilder newBuilder() {
        return new ScaleOutRequestBuilder();
    }

    public ScaleOutRequestBuilder withInstanceName(String instanceName) {
        this.instanceName = instanceName;
        return this;
    }

    public ScaleOutRequestBuilder withAvailabilityZone(String availabilityZone) {
        this.availabilityZone = availabilityZone;
        return this;
    }

    public ScaleOutRequestBuilder withBundle(Bundle bundle) {
        this.bundle = bundle;
        return this;
    }

    public ScaleOutRequestBuilder withAutoscaled(boolean autoscaled) {
        this.autoscaled = autoscaled;
        return this;
    }

    public ScaleOutRequest build() {
        Objects.requireNonNull(instanceName, "InstanceName");
        Objects.requireNonNull(availabilityZone, "AvailabilityZone");
        Objects.requireNonNull(bundle, "Bundle");
        return new ScaleOutRequest(instanceName, availabilityZone, bundle, autoscaled);
    }
}
