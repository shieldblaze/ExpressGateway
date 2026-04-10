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
package com.shieldblaze.expressgateway.lifecycle;

import lombok.Getter;

/**
 * Exception thrown when a lifecycle phase transition fails.
 * Carries the phase and component name for precise diagnostics.
 */
@Getter
public class LifecycleException extends Exception {

    private final LifecyclePhase phase;
    private final String componentName;

    /**
     * Creates a lifecycle exception for a phase-level failure (no specific component).
     *
     * @param phase   the phase that failed
     * @param message the error description
     */
    public LifecycleException(LifecyclePhase phase, String message) {
        super(formatMessage(phase, null, message));
        this.phase = phase;
        this.componentName = null;
    }

    /**
     * Creates a lifecycle exception for a phase-level failure with a cause.
     *
     * @param phase   the phase that failed
     * @param message the error description
     * @param cause   the underlying cause
     */
    public LifecycleException(LifecyclePhase phase, String message, Throwable cause) {
        super(formatMessage(phase, null, message), cause);
        this.phase = phase;
        this.componentName = null;
    }

    /**
     * Creates a lifecycle exception for a component-level failure.
     *
     * @param phase         the phase during which the failure occurred
     * @param componentName the name of the failing component
     * @param message       the error description
     * @param cause         the underlying cause
     */
    public LifecycleException(LifecyclePhase phase, String componentName, String message, Throwable cause) {
        super(formatMessage(phase, componentName, message), cause);
        this.phase = phase;
        this.componentName = componentName;
    }

    private static String formatMessage(LifecyclePhase phase, String componentName, String message) {
        StringBuilder sb = new StringBuilder("Lifecycle failure in phase ")
                .append(phase);
        if (componentName != null) {
            sb.append(" [component=").append(componentName).append("]");
        }
        sb.append(": ").append(message);
        return sb.toString();
    }
}
