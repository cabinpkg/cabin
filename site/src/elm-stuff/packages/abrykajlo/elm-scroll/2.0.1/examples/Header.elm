import Html
import Html.App as App
import Scroll exposing (Move)
import Html exposing (..)
import Html.Attributes exposing (..)
import Animation exposing (px, percent, color)
import Time exposing (second)

import Ports exposing (..)


main =
    App.program
        { init = init
        , subscriptions = subscriptions
        , update = update
        , view = view
        }


type alias Model =
    { style : Animation.State }

  
initialModel =
    { style = Animation.style [ Animation.height (px 90) ] }


init =
    ( initialModel, Cmd.none )


type Action
    = Header Move
    | Shrink
    | Grow
    | Animate Animation.Msg 


update action model =
    case action of
        Grow ->
            let
                style = 
                    Animation.queue
                        [ Animation.toWith
                            (Animation.easing 
                                { duration = 2*second
                                , ease = (\x -> x^2)
                                }
                            ) 
                            [ Animation.height (px 200) ]
                        ]
                    <|
                        Animation.style
                            [ Animation.height (px 90) ]
                newModel = { model | style = style }
            in
                (newModel, Cmd.none)
        Shrink ->
            let
                style = 
                    Animation.queue
                        [ Animation.to
                            [ Animation.height (px 0) ]
                        ]
                    <|
                        Animation.style
                            [ Animation.height (px 90) ]
                newModel = { model | style = style }
            in
                (newModel, Cmd.none)
        Animate animMsg ->
            ({ model
                | style = Animation.update animMsg model.style
            }, Cmd.none)
        Header move ->
            Scroll.handle
                [ update Grow
                    |> Scroll.onCrossDown 400
                , update Shrink
                    |> Scroll.onCrossUp 400
                ]
                move model


view : Model -> Html a
view model =
      div []
        [ div 
          (Animation.render model.style ++ [ style [("position", "fixed")]]) []
        , div [] [] 
        ]


subscriptions : Model -> Sub Action
subscriptions model =
    Sub.batch
        [ scroll Header
        , Animation.subscription Animate [ model.style ]
        ]