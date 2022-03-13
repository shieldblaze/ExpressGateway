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
package com.shieldblaze.expressgateway.integration.aws.lightsail.instance;

public final class ScaleOutRequest {

    private final String instanceName;
    private final String availabilityZone;
    private final Bundle bundle;
    private final boolean autoscaled;

    ScaleOutRequest(String instanceName, String availabilityZone, Bundle bundle, boolean autoscaled) {
        this.instanceName = instanceName;
        this.availabilityZone = availabilityZone;
        this.bundle = bundle;
        this.autoscaled = autoscaled;
    }

    public String instanceName() {
        return instanceName;
    }

    public String availabilityZone() {
        return availabilityZone;
    }

    public Bundle bundle() {
        return bundle;
    }

    public boolean autoscaled() {
        return autoscaled;
    }
}
