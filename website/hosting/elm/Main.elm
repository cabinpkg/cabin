module Main exposing (init, main)

import Browser
import Browser.Navigation exposing (Key)
import Messages exposing (Msg(..))
import Model exposing (Model, Flags, RemoteData(..))
import Routing
import Subscriptions exposing (subscriptions)
import Update exposing (update, loadCurrentPage)
import View exposing (view)
import Url exposing (Url)


init : Flags -> Url -> Key -> ( Model, Cmd Msg )
init flags url navKey =
    let
        model =
            { flags = flags
            , navKey = navKey
            , route = Routing.parseUrl url
            , signinUser = NotRequested
            , signinId = ""
            , otherUser = NotRequested
            , currentToken = NotRequested
            , listPackages = NotRequested
            , detailedPackage = NotRequested
            , search = ""
            , newTokenName = ""
            , isFadein =
                { abstract = False
                , section1 = False
                , demo = False
                , getStart = False
                }
            , searchInput = ""
            , width = 0
            , isChecked = False
            , readme = Nothing
            }
    in
    model
        |> loadCurrentPage


main : Program Flags Model Msg
main =
    Browser.application
        { init = init
        , view = view
        , update = update
        , subscriptions = subscriptions
        , onUrlRequest = OnUrlRequest
        , onUrlChange = OnUrlChange
        }
