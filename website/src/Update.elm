module Update exposing (update, loadCurrentPage)

import Browser
import Browser.Dom exposing (getViewport)
import Browser.Navigation as Nav
import Messages exposing (..)
import Model exposing (..)
import Ports
import Route exposing (Route)
import Task
import Url



update : Msg -> Model -> ( Model, Cmd Msg )
update msg model =
    case msg of
        OnUrlRequest urlRequest ->
            case urlRequest of
                Browser.Internal url ->
                    ( model
                    , Nav.pushUrl model.navKey (Url.toString url)
                    )

                Browser.External url ->
                    ( model
                    , Nav.load url
                    )

        OnUrlChange url ->
            { model | route = Route.fromUrl url }
                |> loadCurrentPage

        OnAnimationFrame _ ->
            ( model, Task.perform GotNewViewport getViewport )

        GotNewViewport viewport ->
            if viewport.viewport.y > 600 then
                ( setTarget asSection1In model True, Cmd.none)
            else if viewport.viewport.y > 200 then
                ( setTarget asGetStartIn model True, Cmd.none)
            else
                ( model, Cmd.none )

        GotNewWidth width ->
            ( { model | width = width }, Cmd.none )

        OnSearchInput searchInput ->
            ( { model | searchInput = searchInput }, Cmd.none )

        Search key ->
            if key == 13 then -- Enter key
                ( model, Route.replaceUrl model.navKey Route.Packages )
            else
                ( model, Cmd.none )

        HandleChecked bool ->
            ( { model | isChecked = bool }, Cmd.none )


loadCurrentPage : Model -> ( Model, Cmd Msg )
loadCurrentPage model =
    case model.route of
        Route.Home ->
            ( model, Ports.suggest () )

        Route.Packages ->
            ( turnOffFadein model, Ports.instantsearch () )

        _ ->
            ( turnOffFadein model, Cmd.none )


setSection1 : Bool -> IsFadein -> IsFadein
setSection1 newBool isFadein =
    { isFadein | section1 = newBool }


asSection1In : IsFadein -> Bool -> IsFadein
asSection1In =
    \b a -> setSection1 a b


setGetStart : Bool -> IsFadein -> IsFadein
setGetStart newBool isFadein =
    { isFadein | getStart = newBool }


asGetStartIn : IsFadein -> Bool -> IsFadein
asGetStartIn =
    \b a -> setGetStart a b


setTarget : (IsFadein -> Bool -> IsFadein) -> Model -> Bool -> Model
setTarget asInFn model newBool =
    newBool
        |> asInFn model.isFadein
        |> asIsFadein model


setIsFadein : IsFadein -> Model -> Model
setIsFadein newIsFadein model =
    { model | isFadein = newIsFadein }


asIsFadein : Model -> IsFadein -> Model
asIsFadein =
    \b a -> setIsFadein a b


setAll : Bool -> IsFadein -> IsFadein
setAll newBool isFadein =
    isFadein
        |> setSection1 newBool
        |> setGetStart newBool


asAll : IsFadein -> Bool -> IsFadein
asAll =
    \b a -> setAll a b


setAllIsFadein : Bool -> Model -> Model
setAllIsFadein newIsFadein model =
    newIsFadein
        |> asAll model.isFadein
        |> asIsFadein model


turnOffFadein : Model -> Model
turnOffFadein model =
    setAllIsFadein False model
