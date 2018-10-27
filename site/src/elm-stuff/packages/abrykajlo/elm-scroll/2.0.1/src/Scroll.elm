module Scroll exposing
    ( Direction(Up, Down), Move, Update
    , onCrossUp, onCrossDown, onCrossOver
    , onUp, onDown, onInRange, onOverlap
    , handle, direction, crossing
    )


{-| This Library provides helper functions for handling events on scrolling.

# Types
@docs Update, Move, Direction

# Creating Scroll Handler
@docs handle

# On Event Handlers
Meant to be used with `Scroll.handle` to build up a list of possible events
for an application. All of the function signatures end in
`Move -> Maybe (Update m msg)` so they can be partially applied and used with
`Scroll.handle`.

## Basic
@docs onUp, onDown

## Complex
@docs onCrossUp, onCrossDown, onCrossOver, onInRange, onOverlap

# Helpers for Creating On Event Handlers
You can create your own as well which can be used with handle as long
they end in `Move -> Maybe (Update m msg)`. These are some functions that
can aid in the creation of them.

@docs direction, crossing
-}


import Maybe
import List exposing (foldl)
import Platform.Cmd exposing (Cmd)

{-| 
    Update m msg == m -> (m, Cmd msg)
-}
type alias Update m msg =
    m -> (m, Cmd msg)


{-| Helps building your own triggers if direction is important.

    upAfterDown : Direction -> Event m msg -> Move ->  Maybe (Event m msg)
    upAfterDown lastDirection event move =
        if direction move != Up then
            Nothing 
        else if lastDirection == Scroll.Down  then
            Just event
        else
            Nothing 
-}
type Direction
    = Up
    | Down


{-| Alias of (Float, Float) represents a move from a scroll position
to another scroll position 

    Move == (from, to)
-}
type alias Move =
    (Float, Float)


{-| A simple helper that returns the direction Up or Down of a Move-}
direction : Move -> Direction
direction (from, to) =
    if from < to then
        Down
    else
        Up


{-| Used in generating a function to trigger all possible events for a single
move. It returns in the standard pattern of `(Model, Effects Action)` the 
updates stack as well, so if many events trigger they update the model in order
they are in the list and the effects get thrown into `Effects.batch`.

    handleEvents : Move -> Model -> (Model, Effects a)
    handleEvents =
        Scroll.handle
            [ update TopBarDrop
              |> Scroll.onUp
            , update ToggleFixBar
              |> Scroll.onCrossOver 400
            ]

    update action model =
        case action of
            Transition move ->
                handleEvents move model
            TopBarDrop clockTime ->
                ...
            ToggleFixBar ->
                ...
-}
handle : List (Move -> Maybe (Update m msg)) -> Move -> m -> (m, Cmd msg)
handle events move model =
    let
        updates =
            List.filterMap (\event -> event move) events
        
        f update (model, cmds) =
            let
                (newModel, cmd) =
                    update model
            in
                (newModel, cmds ++ [cmd])

        (newModel, cmds) =
            foldl f (model, []) updates
    in
        (newModel, Cmd.batch cmds)


{-| Notifies if a `Move` crosses a line. Nothing on no 
crossing and the direction on a crossing-}
crossing : Float -> Move -> Maybe Direction
crossing line (from, to) =
    let
        ratio =
            (line - from) / (to - from)

        crossed =
            0 <= ratio && ratio < 1
    in
        if crossed then
            Just (direction (from, to))
        else
            Nothing


{-| Triggers Update if move crosses the line upwards.

    update Action
    |> onCrossUp line
-}
onCrossUp : Float -> Update m a -> Move -> Maybe (Update m a)
onCrossUp line update move =
    let
        direction =
            crossing line move
    in
        case direction of
            Just Up ->
                Just update

            _ ->
                Nothing


{-| Triggers an update if move crosses the line downwards.

    update Action
    |> onCrossDown line
-}
onCrossDown : Float -> Update m a -> Move -> Maybe (Update m a)
onCrossDown line update move =
    let
        direction =
            crossing line move
    in
        case direction of
            Just Down ->
                Just update

            _ ->
                Nothing


{-| Triggers an update if move crosses the line in either direction.

    update Action
    |> onCrossOver line
-}
onCrossOver : Float -> Update m a -> Move -> Maybe (Update m a)
onCrossOver line update move =
    let
        direction =
            crossing line move
    in
        Maybe.map (\_ -> update) direction


{-| Triggers an update on scrolling upwards 

    update Action
    |> onUp
-}
onUp : Update m a -> Move -> Maybe (Update m a)
onUp update move =
    case direction move of
        Up ->
            Just update
        _ ->
            Nothing


{-| Triggers an update on scrolling downwards

    update Action
    |> onDown
-}
onDown : Update m a -> Move -> Maybe (Update m a)
onDown update move =
    case direction move of
        Down ->
            Just update
        _ ->
            Nothing


{-| Triggers an update if the new scroll position is in
the min max range

    update Action
    |> onInRange min max
-}
onInRange : Float -> Float -> Update m a -> Move -> Maybe (Update m a)
onInRange min max update (_, to) =
    if inRange to min max then
        Just update
    else
        Nothing


inRange : Float -> Float -> Float -> Bool
inRange value min max =
    min <= value && value <= max


{-| Triggers an update if the range of `Move == (from,to)` 
overlaps with the range min max

    update Action
    |> onOverlap min max
-}
onOverlap : Float -> Float -> Update m a -> Move -> Maybe (Update m a)
onOverlap min max update (from, to) =
    if inRange from min max || inRange to min max then
        Just update
    else
        Nothing