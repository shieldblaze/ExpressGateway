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
package com.shieldblaze.expressgateway.protocol.http.translator;

import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;

/**
 * Maps stream lifecycle states across HTTP/1.1, HTTP/2, and HTTP/3.
 *
 * <h3>HTTP/1.1 request-response lifecycle</h3>
 * HTTP/1.1 is serial: a request is sent, a response is received, then the
 * connection is either reused (keep-alive) or closed. There are no explicit
 * "stream states" -- the lifecycle is:
 * <pre>IDLE → REQUEST_SENT → RESPONSE_RECEIVING → COMPLETE</pre>
 *
 * <h3>HTTP/2 stream states (RFC 9113 Section 5.1)</h3>
 * <pre>
 *     idle → open → half-closed(remote) → closed
 *                → half-closed(local)  → closed
 *     idle → reserved(local/remote) → (not used: push is deprecated)
 * </pre>
 *
 * <h3>HTTP/3 stream states (RFC 9114 Section 4.1 + RFC 9000 Section 3.1-3.3)</h3>
 * HTTP/3 delegates stream state management to QUIC. The QUIC stream states are:
 * <pre>
 *     Ready → Send → Data Sent → Data Recvd (sending side)
 *     Recv  → Size Known → Data Recvd → Data Read (receiving side)
 * </pre>
 * At the HTTP/3 level, the relevant events are: HEADERS received, DATA received,
 * FIN (stream close), and RESET_STREAM (stream error).
 *
 * <h3>State mapping across protocols</h3>
 * <table>
 *   <tr><th>State</th><th>H1 equivalent</th><th>H2 state</th><th>H3/QUIC state</th></tr>
 *   <tr><td>IDLE</td><td>Connection ready</td><td>idle</td><td>Ready</td></tr>
 *   <tr><td>OPEN</td><td>Request sent, body sending</td><td>open</td><td>Send + Recv</td></tr>
 *   <tr><td>HALF_CLOSED_LOCAL</td><td>Request complete, awaiting response</td><td>half-closed(local)</td><td>Data Sent + Recv</td></tr>
 *   <tr><td>HALF_CLOSED_REMOTE</td><td>Response complete, still sending</td><td>half-closed(remote)</td><td>Send + Data Recvd</td></tr>
 *   <tr><td>CLOSED</td><td>Exchange complete</td><td>closed</td><td>Data Recvd + Data Read</td></tr>
 *   <tr><td>RESET</td><td>Connection close</td><td>closed (RST_STREAM)</td><td>Reset Sent/Recvd</td></tr>
 * </table>
 *
 * <p>This class provides a unified state tracker that can be used regardless of the
 * source/target protocol pair. The proxy creates one {@code StreamLifecycleMapper}
 * per protocol translation session (connection or stream).</p>
 *
 * <p>Thread safety: uses ConcurrentHashMap for cross-EventLoop access (frontend and
 * backend EventLoops may differ). Individual state transitions are atomic via
 * {@code ConcurrentHashMap.compute}.</p>
 */
public final class StreamLifecycleMapper {

    /**
     * Unified stream states that abstract over protocol-specific state machines.
     */
    public enum StreamState {
        /** Stream is idle/ready, no frames exchanged yet. */
        IDLE,
        /** Stream is open, both sides can send. */
        OPEN,
        /** Local side (proxy→client) has finished sending. Awaiting remote data. */
        HALF_CLOSED_LOCAL,
        /** Remote side (client→proxy) has finished sending. Proxy still responding. */
        HALF_CLOSED_REMOTE,
        /** Stream is fully closed normally. */
        CLOSED,
        /** Stream was reset (RST_STREAM, QUIC RESET_STREAM, or TCP close). */
        RESET
    }

    /**
     * Result of a state transition attempt.
     *
     * @param previousState the state before the transition
     * @param newState      the state after the transition
     * @param valid         whether the transition was valid per the state machine
     */
    public record TransitionResult(StreamState previousState, StreamState newState, boolean valid) {
        /**
         * Returns a result indicating an invalid transition attempt.
         */
        static TransitionResult invalid(StreamState current) {
            return new TransitionResult(current, current, false);
        }
    }

    private final ConcurrentHashMap<Integer, StreamState> states = new ConcurrentHashMap<>();

    /**
     * Registers a new stream in IDLE state.
     *
     * @param streamId the stream identifier (H2 stream ID, H3 stream index, or 0 for H1)
     * @return the initial state (IDLE)
     */
    public StreamState open(int streamId) {
        states.put(streamId, StreamState.IDLE);
        return StreamState.IDLE;
    }

    /**
     * Transitions a stream to OPEN state (both sides can send).
     * Valid from: IDLE.
     *
     * <p>This corresponds to:
     * <ul>
     *   <li>H1: Request sent and body is being transmitted</li>
     *   <li>H2: HEADERS sent without END_STREAM → stream is open (RFC 9113 Section 5.1)</li>
     *   <li>H3: HEADERS sent on QUIC stream, stream is bidirectional</li>
     * </ul>
     *
     * @param streamId the stream identifier
     * @return the transition result
     */
    public TransitionResult activate(int streamId) {
        return transition(streamId, StreamState.IDLE, StreamState.OPEN);
    }

    /**
     * Transitions a stream directly from IDLE to HALF_CLOSED_REMOTE.
     * This handles the case of a GET request with END_STREAM set on the initial
     * HEADERS frame (RFC 9113 Section 5.1), where the remote side immediately
     * signals it has no body to send. The stream skips the OPEN state entirely.
     * Valid from: IDLE.
     *
     * @param streamId the stream identifier
     * @return the transition result
     */
    public TransitionResult activateHalfClosed(int streamId) {
        return transition(streamId, StreamState.IDLE, StreamState.HALF_CLOSED_REMOTE);
    }

    /**
     * Transitions a stream to HALF_CLOSED_LOCAL (local side done sending).
     * Valid from: OPEN.
     *
     * <p>This corresponds to:
     * <ul>
     *   <li>H1: Full request sent (LastHttpContent written)</li>
     *   <li>H2: END_STREAM flag sent on DATA or HEADERS frame (RFC 9113 Section 5.1)</li>
     *   <li>H3: QUIC stream FIN sent (writing side closed)</li>
     * </ul>
     *
     * @param streamId the stream identifier
     * @return the transition result
     */
    public TransitionResult halfCloseLocal(int streamId) {
        return computeTransition(streamId, current -> switch (current) {
            case OPEN -> StreamState.HALF_CLOSED_LOCAL;
            case HALF_CLOSED_REMOTE -> StreamState.CLOSED;
            default -> null; // invalid
        });
    }

    /**
     * Transitions a stream to HALF_CLOSED_REMOTE (remote side done sending).
     * Valid from: OPEN.
     *
     * <p>This corresponds to:
     * <ul>
     *   <li>H1: Full response received (LastHttpContent received)</li>
     *   <li>H2: END_STREAM flag received on DATA or HEADERS frame</li>
     *   <li>H3: QUIC stream FIN received (reading side closed)</li>
     * </ul>
     *
     * @param streamId the stream identifier
     * @return the transition result
     */
    public TransitionResult halfCloseRemote(int streamId) {
        return computeTransition(streamId, current -> switch (current) {
            case OPEN -> StreamState.HALF_CLOSED_REMOTE;
            case HALF_CLOSED_LOCAL -> StreamState.CLOSED;
            default -> null; // invalid
        });
    }

    /**
     * Forces a stream to RESET state (abnormal termination).
     * Valid from any state except CLOSED and RESET.
     *
     * <p>This corresponds to:
     * <ul>
     *   <li>H1: TCP connection close or reset</li>
     *   <li>H2: RST_STREAM frame sent or received</li>
     *   <li>H3: QUIC RESET_STREAM or STOP_SENDING</li>
     * </ul>
     *
     * @param streamId the stream identifier
     * @return the transition result
     */
    public TransitionResult reset(int streamId) {
        return computeTransition(streamId, current -> switch (current) {
            case CLOSED, RESET -> null;
            default -> StreamState.RESET;
        });
    }

    /**
     * Returns the current state of a stream.
     *
     * @param streamId the stream identifier
     * @return the current state, or {@code null} if the stream is not tracked
     */
    public StreamState state(int streamId) {
        return states.get(streamId);
    }

    /**
     * Returns whether a stream can receive data (is in a state where the remote
     * side has not finished sending).
     *
     * @param streamId the stream identifier
     * @return {@code true} if the stream can receive data
     */
    public boolean canReceiveData(int streamId) {
        StreamState state = states.get(streamId);
        return state == StreamState.OPEN || state == StreamState.HALF_CLOSED_LOCAL;
    }

    /**
     * Returns whether a stream can send data (is in a state where the local
     * side has not finished sending).
     *
     * @param streamId the stream identifier
     * @return {@code true} if the stream can send data
     */
    public boolean canSendData(int streamId) {
        StreamState state = states.get(streamId);
        return state == StreamState.OPEN || state == StreamState.HALF_CLOSED_REMOTE;
    }

    /**
     * Removes a stream from the tracker. Should be called after the stream
     * reaches CLOSED or RESET state to prevent unbounded growth.
     *
     * @param streamId the stream identifier
     * @return the final state at removal, or {@code null} if not tracked
     */
    public StreamState remove(int streamId) {
        return states.remove(streamId);
    }

    /**
     * Returns the number of tracked streams.
     */
    public int size() {
        return states.size();
    }

    /**
     * Removes all tracked streams. Used during connection shutdown.
     */
    public void clear() {
        states.clear();
    }

    // ── Internal ─────────────────────────────────────────────────────────────

    private TransitionResult transition(int streamId, StreamState expectedCurrent, StreamState newState) {
        return computeTransition(streamId, current -> current == expectedCurrent ? newState : null);
    }

    /**
     * Atomically computes a state transition. The transitionFn returns the new state
     * if the transition is valid, or null if invalid. Uses ConcurrentHashMap.compute
     * for atomic read-modify-write.
     */
    private TransitionResult computeTransition(int streamId, java.util.function.Function<StreamState, StreamState> transitionFn) {
        StreamState[] result = new StreamState[2]; // [0] = previous, [1] = new (null if invalid)

        states.compute(streamId, (id, current) -> {
            if (current == null) {
                result[0] = null;
                result[1] = null;
                return null;
            }
            StreamState newState = transitionFn.apply(current);
            result[0] = current;
            if (newState == null) {
                result[1] = null;
                return current; // no change
            }
            result[1] = newState;
            return newState;
        });

        if (result[0] == null) {
            return new TransitionResult(StreamState.IDLE, StreamState.IDLE, false);
        }
        if (result[1] == null) {
            return TransitionResult.invalid(result[0]);
        }
        return new TransitionResult(result[0], result[1], true);
    }
}
